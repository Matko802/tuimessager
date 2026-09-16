//! Shared protocol types for tuimessager.
//!
//! This crate is intentionally Discord-free. It defines the REST + WebSocket
//! schema spoken by `tuimessager-server` and `tuimessager-tui`.
//!
//! Ported TUI concepts from Concord (panes, markdown subset, reactions,
//! threads, search, notifications, composer) are mapped onto these types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;

/// A user account on the self-hosted server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct User {
    pub id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub bot: bool,
    pub created_at: DateTime<Utc>,
}

impl User {
    pub fn display(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

/// Presence, simplified from Discord presence to self-hosted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Online,
    Idle,
    Dnd,
    #[default]
    Offline,
}

/// A "server" (Concord: guild). Top-level container in the Servers pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub unread: bool,
    pub mentioned: bool,
}

/// Channel kinds. Threads/forum posts are modelled as channels with a parent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Text,
    Announcement,
    Thread,
    ForumPost,
    VoicePlaceholder,
    Direct,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: Uuid,
    pub server_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub topic: Option<String>,
    pub kind: ChannelKind,
    pub position: i64,
    pub unread_count: u64,
    pub mentioned: bool,
    pub muted: bool,
}

/// A chat message. Markdown subset is rendered client-side (Concord parity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub author: User,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub reply_to: Option<Uuid>,
    pub pinned: bool,
    pub reactions: Vec<ReactionCount>,
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionCount {
    pub emoji: String,
    pub count: u64,
    pub me: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Uuid,
    pub filename: String,
    pub url: String,
    pub mime: Option<String>,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Embed {
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub color: Option<u32>,
}

// ---------------------------------------------------------------------------
// REST DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub name: String,
    pub display_name: Option<String>,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub name: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateServerRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    pub topic: Option<String>,
    pub kind: Option<ChannelKind>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub reply_to: Option<Uuid>,
    /// Idempotency key (Concord: message nonce).
    pub nonce: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EditMessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateMeRequest {
    /// Some(name) sets a new display name (empty clears it); None = no change.
    pub display_name: Option<String>,
    /// Some(password) sets a new password (min 4 chars); None = no change.
    pub password: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetAvatarRequest {
    /// Raw image bytes (PNG, JPEG, GIF or WebP), base64-encoded. Max 1 MiB decoded.
    pub data_base64: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessagesPage {
    pub items: Vec<Message>,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub server_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub author: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub user: User,
    pub presence: Presence,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub message: Message,
    pub kind: NotificationKind,
    pub created_at: DateTime<Utc>,
    pub read: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Mention,
    Reply,
    Direct,
}

// ---------------------------------------------------------------------------
// WebSocket events (server -> client) and commands (client -> server)
// ---------------------------------------------------------------------------

/// Server -> client live events. Replaces Discord gateway events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "d", rename_all = "snake_case")]
pub enum WsEvent {
    Hello { protocol: u32, user: User },
    MessageCreated { message: Message },
    MessageEdited { message: Message },
    MessageDeleted { channel_id: Uuid, id: Uuid },
    ReactionUpdated { channel_id: Uuid, message_id: Uuid, reactions: Vec<ReactionCount> },
    ChannelCreated { channel: Channel },
    Typing { channel_id: Uuid, user: User },
    PresenceUpdated { user_id: Uuid, presence: Presence },
    NotificationCreated { notification: Notification },
    Error { message: String },
}

/// Client -> server commands over WS.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "d", rename_all = "snake_case")]
pub enum WsCommand {
    Subscribe { channel_ids: Vec<Uuid> },
    Typing { channel_id: Uuid },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_event_roundtrip() {
        let id = Uuid::new_v4();
        let ev = WsEvent::MessageDeleted {
            channel_id: id,
            id,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: WsEvent = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, WsEvent::MessageDeleted { .. }));
    }
}
