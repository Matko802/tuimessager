//! tuimessager TUI entrypoint. Concord-style keymap, Discord-free backend.

mod app;
mod client;
mod config;
mod markdown;
mod ui;

use std::{io::Stdout, time::Duration};

use app::{App, Focus, Popup};
use client::Client;
use config::AppOptions;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tuimessager_protocol::WsEvent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut options = config::load_options();
    if let Ok(url) = std::env::var("TUIMESSAGER_URL") {
        if !url.trim().is_empty() {
            options.server_url = url;
        }
    }
    // --check-config parity with Concord.
    if std::env::args().any(|a| a == "--check-config") {
        println!("config OK: {}", config::config_dir().join("config.toml").display());
        return Ok(());
    }
    if std::env::args().any(|a| a == "--init-config") {
        let dir = config::config_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("config.toml"), config::default_config_text())?;
        println!("wrote {}", dir.join("config.toml").display());
        return Ok(());
    }

    let (client, me) = login_flow(&options).await?;
    let mut app = App::new(options, client, me);
    app.refresh_servers().await;
    app.refresh_channels().await;
    app.refresh_notifications().await;

    // WS background task -> mpsc.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WsEvent>();
    let ws_url = app.client.ws_url();
    tokio::spawn(async move {
        loop {
            if ws_loop(&ws_url, &tx).await.is_err() {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    });

    let mut terminal = setup_terminal()?;
    let leader = parse_leader(&config::load_keymap().leader);
    let res = run(&mut terminal, &mut app, &mut rx, leader).await;
    restore_terminal(&mut terminal)?;
    if let Some(msg) = app.exit_message {
        println!("{msg}");
    }
    res
}

async fn ws_loop(url: &str, tx: &tokio::sync::mpsc::UnboundedSender<WsEvent>) -> anyhow::Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(url).await?;
    let (_, mut read) = ws.split();
    while let Some(msg) = read.next().await {
        let msg = msg?;
        if msg.is_text() {
            if let Ok(ev) = serde_json::from_str::<WsEvent>(msg.to_text()?) {
                let _ = tx.send(ev);
            }
        }
    }
    Ok(())
}

async fn login_flow(options: &AppOptions) -> anyhow::Result<(Client, tuimessager_protocol::User)> {
    if let Some(token) = config::load_token(options) {
        let client = Client::new(options.server_url.clone(), token);
        if let Ok(me) = client.me().await {
            return Ok((client, me));
        }
        eprintln!("saved token invalid, please log in again");
    }
    // Simple stdin login (TUI login screen parity without Discord QR/MFA/captcha).
    println!("tuimessager — self-hosted server (no Discord)");
    println!("server: {}", options.server_url);
    let name = prompt("username: ")?;
    let password = rpassword_prompt("password: ")?;
    // Try login, fall back to register.
    match Client::login(&options.server_url, &name, &password).await {
        Ok(auth) => {
            config::save_token(&auth.token)?;
            Ok((Client::new(options.server_url.clone(), auth.token), auth.user))
        }
        Err(_) => {
            println!("login failed, trying register…");
            let auth = Client::register(&options.server_url, &name, &password).await?;
            config::save_token(&auth.token)?;
            Ok((Client::new(options.server_url.clone(), auth.token), auth.user))
        }
    }
}

fn prompt(label: &str) -> anyhow::Result<String> {
    use std::io::Write;
    print!("{label}");
    std::io::stdout().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn rpassword_prompt(label: &str) -> anyhow::Result<String> {
    // Avoid extra dep: disable echo via stty when available.
    #[cfg(unix)]
    {
        use std::io::Write;
        print!("{label}");
        std::io::stdout().flush()?;
        let stty = std::process::Command::new("stty").arg("-echo").stdin(std::process::Stdio::inherit()).status();
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        let _ = std::process::Command::new("stty").arg("echo").stdin(std::process::Stdio::inherit()).status();
        println!();
        let _ = stty;
        return Ok(s.trim().to_string());
    }
    #[cfg(not(unix))]
    {
        return prompt(label);
    }
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<WsEvent>,
    leader: KeyCode,
) -> anyhow::Result<()> {
    let mut leader_pending = false;
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        // Drain WS events without blocking the key poll.
        while let Ok(ev) = rx.try_recv() {
            app.apply_event(ev);
        }
        if !event::poll(Duration::from_millis(80))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        // Ignore key-release events: some terminals report them, and acting
        // on them types/activates everything twice.
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if handle_key(app, key, &mut leader_pending, leader).await? {
            break;
        }
    }
    Ok(())
}

/// Parse the `leader` value from keymap.toml (default "space", Concord parity).
fn parse_leader(s: &str) -> KeyCode {
    match s.trim().to_lowercase().as_str() {
        "space" | " " => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "enter" => KeyCode::Enter,
        s if s.chars().count() == 1 => KeyCode::Char(s.chars().next().unwrap_or(' ')),
        _ => KeyCode::Char(' '),
    }
}

/// Returns true when the app should quit.
async fn handle_key(app: &mut App, key: KeyEvent, leader_pending: &mut bool, leader: KeyCode) -> anyhow::Result<bool> {
    // Composer text input captures most keys.
    if app.composer_open {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                app.composer_open = false;
                app.editing = None;
                app.reply_to = None;
                return Ok(false);
            }
            (KeyCode::Enter, m) if !m.contains(KeyModifiers::SHIFT) => {
                app.submit_composer().await;
                return Ok(false);
            }
            (KeyCode::Backspace, _) => {
                app.composer.pop();
                app.composer_cursor = app.composer.len();
                return Ok(false);
            }
            (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
                app.composer.clear();
                return Ok(false);
            }
            (KeyCode::Char('v'), m) if m.contains(KeyModifiers::CONTROL) => {
                // Paste stub (Concord clipboard parity roadmap).
                return Ok(false);
            }
            (KeyCode::Char(c), _) => {
                app.composer.push(c);
                app.composer_cursor = app.composer.len();
                // Emoji shortcode expansion stub: `:fire:` -> 🔥 on submit handled in send.
                // Mention autocomplete popup on `@`.
                if c == '@' {
                    // keep simple: status hint
                    app.status = "mention: type a name, Tab completion roadmap".to_string();
                }
                return Ok(false);
            }
            _ => return Ok(false),
        }
    }

    // Popup input mode.
    if app.popup != Popup::None {
        // Confirm popups: y/Enter confirms, anything else cancels.
        if app.popup == Popup::ConfirmDelete || app.popup == Popup::ConfirmLogout {
            let confirm = matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter);
            let popup = app.popup;
            app.popup = Popup::None;
            if confirm {
                return confirm_popup(app, popup).await;
            }
            return Ok(false);
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), _) if app.popup_input.is_empty() => {
                app.popup = Popup::None;
                app.popup_input.clear();
                app.popup_idx = 0;
                return Ok(false);
            }
            (KeyCode::Esc, _) => {
                app.popup_input.clear();
                return Ok(false);
            }
            (KeyCode::Enter, _) => {
                activate_popup(app).await;
                return Ok(false);
            }
            (KeyCode::Backspace, _) => {
                app.popup_input.pop();
                app.popup_idx = 0;
                return Ok(false);
            }
            (KeyCode::Up, _) => {
                app.popup_idx = app.popup_idx.saturating_sub(1);
                return Ok(false);
            }
            (KeyCode::Char('k'), m) if m.contains(KeyModifiers::CONTROL) => {
                app.popup_idx = app.popup_idx.saturating_sub(1);
                return Ok(false);
            }
            (KeyCode::Down, _) => {
                app.popup_idx += 1;
                return Ok(false);
            }
            (KeyCode::Char(c), _) => {
                app.popup_input.push(c);
                app.popup_idx = 0;
                return Ok(false);
            }
            _ => return Ok(false),
        }
    }

    // Leader key (configurable via keymap.toml, default Space, Concord parity).
    if *leader_pending {
        *leader_pending = false;
        match key.code {
            KeyCode::Char(' ') => {
                app.popup = Popup::ChannelSwitcher;
                app.popup_input.clear();
                app.popup_idx = 0;
            }
            KeyCode::Char('1') => app.show_servers = !app.show_servers,
            KeyCode::Char('2') => app.show_channels = !app.show_channels,
            KeyCode::Char('4') => app.show_members = !app.show_members,
            KeyCode::Char('a') => app.popup = Popup::MessageActions,
            KeyCode::Char('n') => {
                app.popup = Popup::Inbox;
                app.refresh_notifications().await;
            }
            KeyCode::Char('o') => app.popup = Popup::Help,
            KeyCode::Char('p') => {
                app.status = format!("{} ({})", app.me.display(), app.me.name);
            }
            KeyCode::Char('r') => {
                // Redraw: ratatui handles it next frame.
            }
            KeyCode::Char('v') => {
                app.status = "voice is a UI placeholder in v1 (self-hosted audio roadmap)".to_string();
            }
            KeyCode::Char('l') => {
                app.popup = Popup::ConfirmLogout;
            }
            _ => {}
        }
        return Ok(false);
    }

    match (key.code, key.modifiers) {
        (code, _) if code == leader => {
            *leader_pending = true;
            return Ok(false);
        }
        (KeyCode::Char('q'), _) => {
            if app.focus == Focus::Messages && app.reply_to.is_some() {
                app.reply_to = None;
                return Ok(false);
            }
            return Ok(true);
        }
        (KeyCode::Char('1'), _) => app.focus = Focus::Servers,
        (KeyCode::Char('2'), _) => app.focus = Focus::Channels,
        (KeyCode::Char('3'), _) => app.focus = Focus::Messages,
        (KeyCode::Char('4'), _) => app.focus = Focus::Members,
        (KeyCode::Tab, _) => app.focus = app.focus.next(),
        (KeyCode::BackTab, _) => app.focus = app.focus.prev(),
        (KeyCode::Char('i'), _) => {
            app.composer_open = true;
        }
        (KeyCode::Char('/'), _) => {
            app.popup = Popup::Search;
            app.popup_input.clear();
        }
        (KeyCode::Char(':'), _) => {
            app.popup = Popup::Emoji;
            app.popup_input.clear();
        }
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => move_down(app).await,
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => move_up(app).await,
        (KeyCode::Char('G'), m) if !m.contains(KeyModifiers::SHIFT) || true => {
            // `G` (Shift+g) -> bottom; `gg` simplified to `g` -> top.
            jump_bottom(app).await;
        }
        (KeyCode::Char('g'), _) => jump_top(app).await,
        (KeyCode::Char('y'), _) => {
            if let Some(m) = app.selected_message() {
                app.status = format!("copied {} chars (OSC52 roadmap)", m.content.len());
            }
        }
        (KeyCode::Char('R'), m) if !m.contains(KeyModifiers::SHIFT) || true => {
            if let Some(m) = app.selected_message().cloned() {
                app.reply_to = Some(m.id);
                app.composer_open = true;
            }
        }
        (KeyCode::Char('r'), m) if !m.contains(KeyModifiers::CONTROL) => {
            // `r` react with picker; Ctrl-r is ignored.
            if m.is_empty() {
                app.popup = Popup::Emoji;
                app.popup_input.clear();
            }
        }
        (KeyCode::Char('e'), _) => {
            if let Some(m) = app.selected_message().cloned() {
                if m.author.id == app.me.id {
                    app.editing = Some(m.id);
                    app.composer = m.content;
                    app.composer_open = true;
                } else {
                    app.status = "only your own messages can be edited".to_string();
                }
            }
        }
        (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
            // Ctrl-d half-page down
            for _ in 0..10 {
                move_down(app).await;
            }
        }
        (KeyCode::Char('d'), _) => {
            app.popup = Popup::ConfirmDelete;
        }
        (KeyCode::Char('o'), _) => {
            if let Some(m) = app.selected_message() {
                open_url_in_message(&m.content);
            }
        }
        (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
            for _ in 0..10 {
                move_up(app).await;
            }
        }
        (KeyCode::Enter, _) => {
            // Enter on Servers/Channels selects and loads.
            match app.focus {
                Focus::Servers => app.refresh_channels().await,
                Focus::Channels => app.refresh_history(None).await,
                _ => {}
            }
        }
        _ => {}
    }
    Ok(false)
}

async fn move_down(app: &mut App) {
    match app.focus {
        Focus::Servers => {
            if app.server_idx + 1 < app.servers.len() {
                app.server_idx += 1;
                app.refresh_channels().await;
            }
        }
        Focus::Channels => {
            if app.channel_idx + 1 < app.channels.len() {
                app.channel_idx += 1;
                app.refresh_history(None).await;
            }
        }
        Focus::Messages => {
            if app.msg_idx + 1 < app.messages.len() {
                app.msg_idx += 1;
            }
        }
        Focus::Members => {
            app.member_idx = (app.member_idx + 1).min(app.members.len().saturating_sub(1));
        }
    }
}

async fn move_up(app: &mut App) {
    match app.focus {
        Focus::Servers => {
            app.server_idx = app.server_idx.saturating_sub(1);
            app.refresh_channels().await;
        }
        Focus::Channels => {
            app.channel_idx = app.channel_idx.saturating_sub(1);
            app.refresh_history(None).await;
        }
        Focus::Messages => app.msg_idx = app.msg_idx.saturating_sub(1),
        Focus::Members => app.member_idx = app.member_idx.saturating_sub(1),
    }
}

async fn jump_top(app: &mut App) {
    match app.focus {
        Focus::Servers => {
            app.server_idx = 0;
            app.refresh_channels().await;
        }
        Focus::Channels => {
            app.channel_idx = 0;
            app.refresh_history(None).await;
        }
        Focus::Messages => app.msg_idx = 0,
        Focus::Members => app.member_idx = 0,
    }
}

async fn jump_bottom(app: &mut App) {
    match app.focus {
        Focus::Servers => {
            app.server_idx = app.servers.len().saturating_sub(1);
            app.refresh_channels().await;
        }
        Focus::Channels => {
            app.channel_idx = app.channels.len().saturating_sub(1);
            app.refresh_history(None).await;
        }
        Focus::Messages => app.msg_idx = app.messages.len().saturating_sub(1),
        Focus::Members => app.member_idx = app.members.len().saturating_sub(1),
    }
}

/// Confirm handler for ConfirmDelete / ConfirmLogout.
/// Returns true when the app should quit (logout).
async fn confirm_popup(app: &mut App, popup: Popup) -> anyhow::Result<bool> {
    match popup {
        Popup::ConfirmDelete => {
            if let Some(m) = app.selected_message().cloned() {
                if m.author.id != app.me.id {
                    app.status = "only your own messages can be deleted".to_string();
                    return Ok(false);
                }
                match app.client.delete(m.id).await {
                    Ok(()) => {
                        app.messages.retain(|x| x.id != m.id);
                        app.msg_idx = app.msg_idx.min(app.messages.len().saturating_sub(1));
                    }
                    Err(e) => app.status = format!("delete: {e:#}"),
                }
            }
            Ok(false)
        }
        Popup::ConfirmLogout => {
            // Best-effort server-side invalidation, then drop the local token.
            if let Err(e) = app.client.logout().await {
                app.status = format!("logout: {e:#} (clearing local token anyway)");
            }
            crate::config::clear_token();
            app.exit_message = Some("Logged out of tuimessager.".to_string());
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn activate_popup(app: &mut App) {
    match app.popup {
        Popup::ChannelSwitcher => {
            if let Some((_, ci)) = app.switcher_items().get(app.popup_idx) {
                app.channel_idx = *ci;
                app.focus = Focus::Messages;
                app.refresh_history(None).await;
            }
            app.popup = Popup::None;
            app.popup_input.clear();
        }
        Popup::Search => {
            let q = app.popup_input.clone();
            let ch = app.selected_channel().map(|c| c.id);
            match app.client.search(&q, ch).await {
                Ok(results) => {
                    app.messages = results;
                    app.msg_idx = 0;
                    app.focus = Focus::Messages;
                    app.status = format!("search: {} hits", app.messages.len());
                }
                Err(e) => app.status = format!("search: {e:#}"),
            }
            app.popup = Popup::None;
            app.popup_input.clear();
        }
        Popup::Emoji => {
            let emojis = ["🔥", "👍", "❤️", "😂", "🎉", "😮", "😢", "🙏", "👀", "💯"];
            if let Some(e) = emojis.get(app.popup_idx) {
                // If composer open, insert; else react to selected message.
                if app.composer_open {
                    app.composer.push_str(e);
                } else if let Some(m) = app.selected_message().cloned() {
                    match app.client.react(m.id, e).await {
                        Ok(updated) => {
                            if let Some(pos) = app.messages.iter().position(|x| x.id == m.id) {
                                app.messages[pos] = updated;
                            }
                        }
                        Err(e) => app.status = format!("react: {e:#}"),
                    }
                }
            }
            app.popup = Popup::None;
            app.popup_input.clear();
        }
        Popup::Inbox => {
            app.popup = Popup::None;
        }
        _ => {
            app.popup = Popup::None;
        }
    }
    app.popup_idx = 0;
}

fn open_url_in_message(content: &str) {
    let url = content.split_whitespace().find(|w| w.starts_with("http://") || w.starts_with("https://"));
    if let Some(u) = url {
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open").arg(u).spawn();
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(u).spawn();
    }
}
