use std::{net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_path: PathBuf,
    pub allow_registration: bool,
}

impl Config {
    pub fn from_env() -> Self {
        let bind = std::env::var("TUIMESSAGER_BIND")
            .unwrap_or_else(|_| "0.0.0.0:3000".to_string())
            .parse()
            .expect("invalid TUIMESSAGER_BIND (expected ip:port)");
        let database_path = std::env::var("TUIMESSAGER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("tuimessager.db");
        // Also allow direct file override.
        let database_path = std::env::var("TUIMESSAGER_DATABASE_PATH")
            .map(PathBuf::from)
            .unwrap_or(database_path);
        let allow_registration = std::env::var("TUIMESSAGER_ALLOW_REGISTRATION")
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(true);
        Self {
            bind,
            database_path,
            allow_registration,
        }
    }
}
