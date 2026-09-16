//! Config ported from Concord (Discord-free).
//! Files live in `$XDG_CONFIG_HOME/tuimessager/` or `~/.config/tuimessager/`:
//! `config.toml`, `keymap.toml`, `theme.toml`.
//! State (token fallback) lives in `$XDG_STATE_HOME/tuimessager/`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config")))
        .join("tuimessager")
}

pub fn state_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::state_dir().unwrap_or_else(|| PathBuf::from("~/.local/state")))
        .join("tuimessager")
}

pub fn token_file() -> PathBuf {
    state_dir().join("token")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppOptions {
    pub server_url: String,
    pub display: DisplayOptions,
    pub composer: ComposerOptions,
    pub notifications: NotificationOptions,
    pub credentials: CredentialOptions,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:3000".to_string(),
            display: DisplayOptions::default(),
            composer: ComposerOptions::default(),
            notifications: NotificationOptions::default(),
            credentials: CredentialOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayOptions {
    pub show_avatars: bool,
    pub show_images: bool,
    pub image_protocol: String,
    pub hour_format_24: bool,
    pub circular_avatars: bool,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self {
            show_avatars: true,
            show_images: true,
            image_protocol: "auto".to_string(),
            hour_format_24: true,
            circular_avatars: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ComposerOptions {
    pub ping_on_reply: bool,
}

impl Default for ComposerOptions {
    fn default() -> Self {
        Self { ping_on_reply: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationOptions {
    pub desktop_notifications: bool,
}

impl Default for NotificationOptions {
    fn default() -> Self {
        Self { desktop_notifications: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CredentialOptions {
    /// Where to keep the opaque server token: `auto` (env > file) or `plain`.
    pub store: String,
}

impl Default for CredentialOptions {
    fn default() -> Self {
        Self { store: "auto".to_string() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Keymap {
    #[serde(default = "default_leader")]
    pub leader: String,
}

fn default_leader() -> String {
    "space".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Theme {
    #[serde(default)]
    pub selection_marker: String,
}

impl Theme {
    pub fn selection_marker_or_default(&self) -> &str {
        if self.selection_marker.is_empty() {
            "▸ "
        } else {
            &self.selection_marker
        }
    }
}

pub fn load_options() -> AppOptions {
    let path = config_dir().join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return AppOptions::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

pub fn load_keymap() -> Keymap {
    let path = config_dir().join("keymap.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Keymap::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

pub fn load_theme() -> Theme {
    let path = config_dir().join("theme.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Theme::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

pub fn load_token(options: &AppOptions) -> Option<String> {
    if let Ok(t) = std::env::var("TUIMESSAGER_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t);
        }
    }
    if options.credentials.store == "auto" || options.credentials.store == "plain" {
        if let Ok(t) = std::fs::read_to_string(token_file()) {
            let t = t.trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    if let Ok(url) = std::env::var("TUIMESSAGER_URL") {
        let _ = url;
    }
    None
}

pub fn save_token(token: &str) -> anyhow::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
    }
    std::fs::write(token_file(), token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(token_file(), std::fs::Permissions::from_mode(0o600)).ok();
    }
    Ok(())
}

pub fn clear_token() {
    std::fs::remove_file(token_file()).ok();
}

pub fn default_config_text() -> &'static str {
    r#"# tuimessager config (Concord-style, Discord-free)
server_url = "http://127.0.0.1:3000"

[display]
show_avatars = true
show_images = true
image_protocol = "auto"
hour_format_24 = true
circular_avatars = false

[composer]
ping_on_reply = true

[notifications]
desktop_notifications = true

[credentials]
store = "auto"
"#
}
