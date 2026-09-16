mod auth;
mod config;
mod db;
mod routes;

use std::sync::Arc;

use tokio::sync::{Mutex, broadcast};
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tuimessager_protocol::WsEvent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "tuimessager_server=info,tower_http=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cfg = config::Config::from_env();
    tracing::info!("opening database at {}", cfg.database_path.display());
    let conn = db::open(&cfg.database_path)?;
    let (tx, _) = broadcast::channel::<WsEvent>(256);
    let state = routes::AppState {
        db: Arc::new(Mutex::new(conn)),
        events: tx,
        allow_registration: cfg.allow_registration,
    };

    let app = routes::router(state).layer(CorsLayer::permissive());

    tracing::info!("tuimessager-server listening on {}", cfg.bind);
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
