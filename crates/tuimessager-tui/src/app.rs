//! App state: Concord-style dashboard (Servers | Channels | Messages | Members)
//! with leader key, fuzzy switcher, inbox, search, composer, emoji picker.
//! Backend is tuimessager-server (no Discord gateway/REST/RPC/voice).

use chrono::{Local, TimeZone};
use tuimessager_protocol::*;
use uuid::Uuid;

use crate::{client::Client, config::{AppOptions, Theme}, markdown::fuzzy_score};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Servers,
    Channels,
    Messages,
    Members,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Self::Servers => Self::Channels,
            Self::Channels => Self::Messages,
            Self::Messages => Self::Members,
            Self::Members => Self::Servers,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Self::Servers => Self::Members,
            Self::Channels => Self::Servers,
            Self::Messages => Self::Channels,
            Self::Members => Self::Messages,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    None,
    Help,
    ChannelSwitcher,
    Search,
    Inbox,
    Emoji,
    ConfirmDelete,
    ConfirmLogout,
    MessageActions,
}

pub struct App {
    pub options: AppOptions,
    pub client: Client,
    pub me: User,
    pub servers: Vec<Server>,
    pub channels: Vec<Channel>,
    pub members: Vec<Member>,
    pub messages: Vec<Message>,
    pub notifications: Vec<Notification>,
    pub focus: Focus,
    pub server_idx: usize,
    pub channel_idx: usize,
    pub msg_idx: usize,
    pub member_idx: usize,
    pub msg_scroll: usize,
    pub composer_open: bool,
    pub composer: String,
    pub composer_cursor: usize,
    pub reply_to: Option<Uuid>,
    pub editing: Option<Uuid>,
    pub popup: Popup,
    pub popup_input: String,
    pub popup_idx: usize,
    pub status: String,
    pub show_members: bool,
    pub show_servers: bool,
    pub show_channels: bool,
    pub has_more: bool,
    pub theme: Theme,
    /// Message printed after the terminal is restored (e.g. "Logged out").
    pub exit_message: Option<String>,
}

impl App {
    pub fn new(options: AppOptions, client: Client, me: User) -> Self {
        Self {
            options,
            client,
            me,
            servers: vec![],
            channels: vec![],
            members: vec![],
            messages: vec![],
            notifications: vec![],
            focus: Focus::Servers,
            server_idx: 0,
            channel_idx: 0,
            msg_idx: 0,
            member_idx: 0,
            msg_scroll: 0,
            composer_open: false,
            composer: String::new(),
            composer_cursor: 0,
            reply_to: None,
            editing: None,
            popup: Popup::None,
            popup_input: String::new(),
            popup_idx: 0,
            status: "Connecting… (Space for shortcuts, / search, i compose)".to_string(),
            show_members: true,
            show_servers: true,
            show_channels: true,
            has_more: false,
            theme: crate::config::load_theme(),
            exit_message: None,
        }
    }

    pub fn selected_server(&self) -> Option<&Server> {
        self.servers.get(self.server_idx)
    }
    pub fn selected_channel(&self) -> Option<&Channel> {
        self.channels.get(self.channel_idx)
    }
    pub fn selected_message(&self) -> Option<&Message> {
        self.messages.get(self.msg_idx)
    }

    pub async fn refresh_servers(&mut self) {
        match self.client.servers().await {
            Ok(s) => {
                self.servers = s;
                self.server_idx = self.server_idx.min(self.servers.len().saturating_sub(1));
                self.status.clear();
            }
            Err(e) => self.status = format!("servers: {e:#}"),
        }
    }

    pub async fn refresh_channels(&mut self) {
        let Some(s) = self.selected_server().cloned() else { return };
        match self.client.channels(s.id).await {
            Ok(c) => {
                self.channels = c;
                self.channel_idx = 0;
                self.refresh_members().await;
                self.refresh_history(None).await;
            }
            Err(e) => self.status = format!("channels: {e:#}"),
        }
    }

    pub async fn refresh_members(&mut self) {
        let Some(s) = self.selected_server().cloned() else { return };
        if let Ok(m) = self.client.members(s.id).await {
            self.members = m;
        }
    }

    pub async fn refresh_history(&mut self, before: Option<Uuid>) {
        let Some(c) = self.selected_channel().cloned() else { return };
        match self.client.history(c.id, before, 50).await {
            Ok(page) => {
                if before.is_some() {
                    let mut older = page.items;
                    older.extend(std::mem::take(&mut self.messages));
                    self.messages = older;
                } else {
                    self.messages = page.items;
                    self.msg_idx = self.messages.len().saturating_sub(1);
                }
                self.has_more = page.has_more;
                self.status.clear();
            }
            Err(e) => self.status = format!("history: {e:#}"),
        }
    }

    pub async fn refresh_notifications(&mut self) {
        if let Ok(n) = self.client.notifications().await {
            let unread = n.iter().filter(|x| !x.read).count();
            if unread > 0 && self.options.notifications.desktop_notifications {
                // Best-effort desktop toast for the newest unread.
                if let Some(first) = n.iter().find(|x| !x.read) {
                    let _ = notify_rust::Notification::new()
                        .summary(&format!("tuimessager: {}", first.message.author.display()))
                        .body(&first.message.content.chars().take(200).collect::<String>())
                        .show();
                }
            }
            self.notifications = n;
        }
    }

    pub async fn submit_composer(&mut self) {
        let text = self.composer.trim().to_string();
        if text.is_empty() {
            self.composer_open = false;
            return;
        }
        let Some(ch) = self.selected_channel().cloned() else { return };
        if let Some(edit_id) = self.editing.take() {
            match self.client.edit(edit_id, text).await {
                Ok(m) => {
                    if let Some(pos) = self.messages.iter().position(|x| x.id == edit_id) {
                        self.messages[pos] = m;
                    }
                    self.status.clear();
                }
                Err(e) => self.status = format!("edit: {e:#}"),
            }
        } else {
            match self.client.send(ch.id, text, self.reply_to).await {
                Ok(m) => {
                    self.messages.push(m);
                    self.msg_idx = self.messages.len().saturating_sub(1);
                    self.status.clear();
                }
                Err(e) => self.status = format!("send: {e:#}"),
            }
        }
        self.composer.clear();
        self.composer_cursor = 0;
        self.reply_to = None;
        self.composer_open = false;
    }

    pub fn apply_event(&mut self, ev: WsEvent) {
        match ev {
            WsEvent::MessageCreated { message } => {
                if self.selected_channel().map(|c| c.id) == Some(message.channel_id) {
                    self.messages.push(message);
                    if self.focus == Focus::Messages {
                        self.msg_idx = self.messages.len().saturating_sub(1);
                    }
                } else {
                    self.status = format!("new message in {}", message.channel_id);
                }
            }
            WsEvent::MessageEdited { message } => {
                if let Some(pos) = self.messages.iter().position(|m| m.id == message.id) {
                    self.messages[pos] = message;
                }
            }
            WsEvent::MessageDeleted { id, .. } => {
                self.messages.retain(|m| m.id != id);
                self.msg_idx = self.msg_idx.min(self.messages.len().saturating_sub(1));
            }
            WsEvent::ReactionUpdated { message_id, reactions, .. } => {
                if let Some(m) = self.messages.iter_mut().find(|m| m.id == message_id) {
                    m.reactions = reactions;
                }
            }
            WsEvent::ChannelCreated { channel } => {
                // Refresh lazily to keep ordering.
                self.status = format!("new channel: #{}", channel.name);
            }
            WsEvent::NotificationCreated { notification } => {
                self.notifications.insert(0, notification);
            }
            WsEvent::Hello { .. } | WsEvent::Typing { .. } | WsEvent::PresenceUpdated { .. } | WsEvent::Error { .. } => {}
        }
    }

    // --- filtering helpers (Concord `/` + `Space Space`) ---

    pub fn switcher_items(&self) -> Vec<(i64, usize)> {
        let q = self.popup_input.trim();
        let mut scored: Vec<(i64, usize)> = self
            .channels
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let label = if q.starts_with('*') {
                    format!("{} #{}", self.servers.get(self.server_idx).map(|s| s.name.as_str()).unwrap_or(""), c.name)
                } else {
                    format!("#{}", c.name)
                };
                let qq = q.trim_start_matches('*').trim();
                fuzzy_score(qq, &label).map(|s| (s, i))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.truncate(20);
        scored
    }

    pub fn format_time(&self, dt: &chrono::DateTime<chrono::Utc>) -> String {
        let local = Local.from_utc_datetime(&dt.naive_utc());
        if self.options.display.hour_format_24 {
            local.format("%H:%M").to_string()
        } else {
            local.format("%I:%M %p").to_string()
        }
    }
}
