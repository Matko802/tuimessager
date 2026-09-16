use std::sync::Arc;

use axum::{
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{Message as AxumWsMessage, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, patch, post},
    Router,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use tokio::sync::{Mutex, broadcast};
use tuimessager_protocol::*;
use uuid::Uuid;

use crate::{auth, db};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<rusqlite::Connection>>,
    pub events: broadcast::Sender<WsEvent>,
    pub allow_registration: bool,
    pub data_dir: std::path::PathBuf,
}

fn avatar_path(state: &AppState, user_id: &str) -> std::path::PathBuf {
    state.data_dir.join("avatars").join(user_id)
}

type ApiResult<T> = Result<T, (StatusCode, String)>;

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, String) {
    (status, msg.into())
}

// --- auth helper ---

pub async fn auth_user(state: &AppState, headers: &HeaderMap) -> Result<User, (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing Bearer token"))?;
    let conn = state.db.lock().await;
    let user_id: Option<String> = conn
        .query_row("SELECT user_id FROM tokens WHERE token=?1", params![token], |r| r.get(0))
        .optional()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(uid) = user_id else {
        return Err(err(StatusCode::UNAUTHORIZED, "invalid token"));
    };
    let user = db::user_by_id(&conn, &uid)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "unknown user"))?;
    Ok(user)
}

fn new_token() -> String {
    // Opaque token, no Discord-style secrets. Stored hashed? Plain is fine for v1
    // behind trusted network; recommend TLS reverse proxy for internet exposure.
    format!("tm_{}", Uuid::new_v4().simple())
}

// --- handlers ---

async fn health() -> &'static str {
    "ok"
}

async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !state.allow_registration {
        return Err(err(StatusCode::FORBIDDEN, "registration disabled"));
    }
    if req.name.trim().is_empty() || req.password.len() < 4 {
        return Err(err(StatusCode::BAD_REQUEST, "name/password too short"));
    }
    if req.name.len() > 32 {
        return Err(err(StatusCode::BAD_REQUEST, "name too long"));
    }
    let hash = auth::hash_password(&req.password)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let conn = state.db.lock().await;
    let exists: Option<String> = conn
        .query_row("SELECT id FROM users WHERE name=?1", params![req.name], |r| r.get(0))
        .optional()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if exists.is_some() {
        return Err(err(StatusCode::CONFLICT, "name taken"));
    }
    let id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO users (id, name, display_name, password_hash, bot, created_at) VALUES (?1,?2,?3,?4,0,?5)",
        params![id.to_string(), req.name, req.display_name, hash, now],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let token = new_token();
    conn.execute(
        "INSERT INTO tokens (token, user_id, created_at) VALUES (?1,?2,?3)",
        params![token, id.to_string(), now],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Auto-join seeded servers so a fresh account sees content immediately.
    let server_ids: Vec<String> = conn
        .prepare("SELECT id FROM servers")
        .and_then(|mut s| s.query_map([], |r| r.get(0)).and_then(|rows| rows.collect::<Result<Vec<_>, _>>()))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    for sid in server_ids {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO server_members (server_id, user_id) VALUES (?1,?2)",
            params![sid, id.to_string()],
        );
    }
    let user = db::user_by_id(&conn, &id.to_string())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap();
    Ok((StatusCode::CREATED, Json(AuthResponse { token, user })))
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let conn = state.db.lock().await;
    let Some((user, hash)) = db::user_by_name(&conn, &req.name)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    else {
        return Err(err(StatusCode::UNAUTHORIZED, "invalid credentials"));
    };
    if !auth::verify_password(&hash, &req.password) {
        return Err(err(StatusCode::UNAUTHORIZED, "invalid credentials"));
    }
    let token = new_token();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO tokens (token, user_id, created_at) VALUES (?1,?2,?3)",
        params![token, user.id.to_string(), now],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(AuthResponse { token, user }))
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<User>> {
    Ok(Json(auth_user(&state, &headers).await?))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<impl IntoResponse> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing Bearer token"))?;
    // Validate first so we return 401 (not success) for bogus tokens.
    let _ = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    conn.execute("DELETE FROM tokens WHERE token=?1", params![token])
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpdateMeRequest>,
) -> ApiResult<Json<User>> {
    let user = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    if let Some(name) = req.display_name {
        if name.len() > 64 {
            return Err(err(StatusCode::BAD_REQUEST, "display name too long"));
        }
        let name: Option<String> = (!name.trim().is_empty()).then_some(name.trim().to_string());
        conn.execute("UPDATE users SET display_name=?1 WHERE id=?2", params![name, user.id.to_string()])
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    if let Some(password) = req.password {
        if password.len() < 4 {
            return Err(err(StatusCode::BAD_REQUEST, "password too short"));
        }
        let hash = auth::hash_password(&password)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        conn.execute("UPDATE users SET password_hash=?1 WHERE id=?2", params![hash, user.id.to_string()])
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        // Changing the password kills all other sessions (not the current one).
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .unwrap_or("");
        conn.execute(
            "DELETE FROM tokens WHERE user_id=?1 AND token!=?2",
            params![user.id.to_string(), token],
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    let user = db::user_by_id(&conn, &user.id.to_string())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap();
    Ok(Json(user))
}

fn sniff_avatar_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

async fn set_avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SetAvatarRequest>,
) -> ApiResult<Json<User>> {
    use base64::Engine;
    let user = auth_user(&state, &headers).await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(req.data_base64.trim())
        .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid base64"))?;
    if bytes.is_empty() || bytes.len() > 1_000_000 {
        return Err(err(StatusCode::BAD_REQUEST, "image must be 1 byte–1 MiB"));
    }
    if sniff_avatar_mime(&bytes).is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "image must be PNG, JPEG, GIF or WebP"));
    }
    let path = avatar_path(&state, &user.id.to_string());
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let conn = state.db.lock().await;
    conn.execute("UPDATE users SET avatar=1 WHERE id=?1", params![user.id.to_string()])
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let user = db::user_by_id(&conn, &user.id.to_string())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap();
    Ok(Json(user))
}

async fn delete_avatar(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<impl IntoResponse> {
    let user = auth_user(&state, &headers).await?;
    let _ = tokio::fs::remove_file(avatar_path(&state, &user.id.to_string())).await;
    let conn = state.db.lock().await;
    conn.execute("UPDATE users SET avatar=0 WHERE id=?1", params![user.id.to_string()])
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_avatar(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Public (like most chat CDNs): small images, cacheable for a day.
    let bytes = tokio::fs::read(avatar_path(&state, &id.to_string()))
        .await
        .map_err(|_| err(StatusCode::NOT_FOUND, "no avatar"))?;
    let mime = sniff_avatar_mime(&bytes).unwrap_or("application/octet-stream");
    let mut headers = HeaderMap::new();
    headers.insert("content-type", mime.parse().unwrap());
    headers.insert("cache-control", "public, max-age=86400".parse().unwrap());
    Ok((headers, bytes))
}

async fn list_servers(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<Server>>> {
    let user = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    let mut stmt = conn
        .prepare("SELECT s.id, s.name, s.description, s.created_at FROM servers s JOIN server_members m ON m.server_id=s.id WHERE m.user_id=?1 ORDER BY s.name")
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rows = stmt
        .query_map(params![user.id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        let (id, name, description, created_at) = r.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let unread: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages msg JOIN channels c ON c.id=msg.channel_id WHERE c.server_id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let _ = unread;
        out.push(Server {
            id: id.parse().unwrap_or_else(|_| Uuid::nil()),
            name,
            description,
            created_at: created_at.parse().unwrap_or_else(|_| Utc::now()),
            unread: false,
            mentioned: false,
        });
    }
    Ok(Json(out))
}

async fn create_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateServerRequest>,
) -> ApiResult<impl IntoResponse> {
    let user = auth_user(&state, &headers).await?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    let conn = state.db.lock().await;
    let id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO servers (id, name, description, created_at) VALUES (?1,?2,?3,?4)",
        params![id.to_string(), req.name, req.description, now],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    conn.execute(
        "INSERT INTO server_members (server_id, user_id) VALUES (?1,?2)",
        params![id.to_string(), user.id.to_string()],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let general = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO channels (id, server_id, name, kind, position) VALUES (?1,?2,'general','text',0)",
        params![general, id.to_string()],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((
        StatusCode::CREATED,
        Json(Server {
            id,
            name: req.name,
            description: req.description,
            created_at: Utc::now(),
            unread: false,
            mentioned: false,
        }),
    ))
}

async fn list_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Channel>>> {
    let _ = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    let mut stmt = conn
        .prepare("SELECT id, server_id, parent_id, name, topic, kind, position FROM channels WHERE server_id=?1 ORDER BY position, name")
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rows = stmt
        .query_map(params![server_id.to_string()], |r| {
            Ok(Channel {
                id: r.get::<_, String>(0)?.parse().unwrap_or_else(|_| Uuid::nil()),
                server_id: r.get::<_, Option<String>>(1)?.and_then(|s| s.parse().ok()),
                parent_id: r.get::<_, Option<String>>(2)?.and_then(|s| s.parse().ok()),
                name: r.get(3)?,
                topic: r.get(4)?,
                kind: match r.get::<_, String>(5)?.as_str() {
                    "thread" => ChannelKind::Thread,
                    "forum_post" => ChannelKind::ForumPost,
                    "direct" => ChannelKind::Direct,
                    "announcement" => ChannelKind::Announcement,
                    "voice_placeholder" => ChannelKind::VoicePlaceholder,
                    _ => ChannelKind::Text,
                },
                position: r.get(6)?,
                unread_count: 0,
                mentioned: false,
                muted: false,
            })
        })
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }
    Ok(Json(out))
}

async fn create_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server_id): Path<Uuid>,
    Json(req): Json<CreateChannelRequest>,
) -> ApiResult<impl IntoResponse> {
    let _ = auth_user(&state, &headers).await?;
    if req.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    let conn = state.db.lock().await;
    let id = Uuid::new_v4();
    let kind = match req.kind.unwrap_or(ChannelKind::Text) {
        ChannelKind::Thread => "thread",
        ChannelKind::ForumPost => "forum_post",
        ChannelKind::Direct => "direct",
        ChannelKind::Announcement => "announcement",
        ChannelKind::VoicePlaceholder => "voice_placeholder",
        ChannelKind::Text => "text",
    };
    conn.execute(
        "INSERT INTO channels (id, server_id, name, topic, kind, position) VALUES (?1,?2,?3,?4,?5,0)",
        params![id.to_string(), server_id.to_string(), req.name, req.topic, kind],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let channel = Channel {
        id,
        server_id: Some(server_id),
        parent_id: None,
        name: req.name,
        topic: req.topic,
        kind: req.kind.unwrap_or(ChannelKind::Text),
        position: 0,
        unread_count: 0,
        mentioned: false,
        muted: false,
    };
    let _ = state.events.send(WsEvent::ChannelCreated { channel: channel.clone() });
    Ok((StatusCode::CREATED, Json(channel)))
}

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<i64>,
    before: Option<String>,
}

async fn channel_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<Uuid>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<MessagesPage>> {
    let user = auth_user(&state, &headers).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let conn = state.db.lock().await;
    let mut sql = String::from(
        "SELECT id FROM messages WHERE channel_id=?1",
    );
    if q.before.is_some() {
        sql.push_str(" AND created_at < (SELECT created_at FROM messages WHERE id=?2)");
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?3");
    let ids: Vec<String> = if let Some(before) = q.before {
        conn.prepare(&sql)
            .and_then(|mut s| {
                s.query_map(params![channel_id.to_string(), before, limit + 1], |r| r.get(0))
                    .and_then(|rows| rows.collect::<Result<Vec<String>, _>>())
            })
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        conn.prepare("SELECT id FROM messages WHERE channel_id=?1 ORDER BY created_at DESC LIMIT ?2")
            .and_then(|mut s| {
                s.query_map(params![channel_id.to_string(), limit + 1], |r| r.get(0))
                    .and_then(|rows| rows.collect::<Result<Vec<String>, _>>())
            })
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let has_more = ids.len() as i64 > limit;
    let mut items = Vec::new();
    for id in ids.into_iter().take(limit as usize) {
        if let Some(m) = db::hydrate_message(&conn, &id, &user.id.to_string())
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            items.push(m);
        }
    }
    items.reverse();
    Ok(Json(MessagesPage { items, has_more }))
}

async fn send_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<Uuid>,
    Json(req): Json<SendMessageRequest>,
) -> ApiResult<impl IntoResponse> {
    let user = auth_user(&state, &headers).await?;
    if req.content.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "empty message"));
    }
    if req.content.len() > 8000 {
        return Err(err(StatusCode::BAD_REQUEST, "message too long"));
    }
    let conn = state.db.lock().await;
    // Idempotency via nonce (Concord parity).
    if let Some(nonce) = req.nonce {
        let existing: Option<String> = conn
            .query_row("SELECT id FROM messages WHERE nonce=?1", params![nonce.to_string()], |r| r.get(0))
            .optional()
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(id) = existing {
            if let Some(m) = db::hydrate_message(&conn, &id, &user.id.to_string())
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            {
                return Ok((StatusCode::OK, Json(m)));
            }
        }
    }
    let id = Uuid::new_v4();
    let now = Utc::now();
    conn.execute(
        "INSERT INTO messages (id, channel_id, author_id, content, created_at, reply_to, nonce) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            id.to_string(),
            channel_id.to_string(),
            user.id.to_string(),
            req.content,
            now.to_rfc3339(),
            req.reply_to.map(|u| u.to_string()),
            req.nonce.map(|u| u.to_string()),
        ],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let msg = db::hydrate_message(&conn, &id.to_string(), &user.id.to_string())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap();
    // Naive mention detection: @name substring.
    detect_mentions(&conn, &msg)?;
    let _ = state.events.send(WsEvent::MessageCreated { message: msg.clone() });
    Ok((StatusCode::CREATED, Json(msg)))
}

fn detect_mentions(conn: &rusqlite::Connection, msg: &Message) -> Result<(), (StatusCode, String)> {
    // For every user whose @name appears, create a Mention notification.
    let users: Vec<(String, String)> = conn
        .prepare("SELECT id, name FROM users")
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        })
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    for (uid, name) in users {
        if uid == msg.author.id.to_string() {
            continue;
        }
        if msg.content.contains(&format!("@{name}")) {
            let nid = Uuid::new_v4().to_string();
            let _ = conn.execute(
                "INSERT INTO notifications (id, user_id, channel_id, message_id, kind, created_at, read) VALUES (?1,?2,?3,?4,'mention',?5,0)",
                params![nid, uid, msg.channel_id.to_string(), msg.id.to_string(), Utc::now().to_rfc3339()],
            );
        }
    }
    Ok(())
}

async fn edit_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<EditMessageRequest>,
) -> ApiResult<Json<Message>> {
    let user = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    let author: Option<String> = conn
        .query_row("SELECT author_id FROM messages WHERE id=?1", params![id.to_string()], |r| r.get(0))
        .optional()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match author {
        Some(a) if a == user.id.to_string() => {},
        Some(_) => return Err(err(StatusCode::FORBIDDEN, "not your message")),
        None => return Err(err(StatusCode::NOT_FOUND, "not found")),
    }
    conn.execute(
        "UPDATE messages SET content=?1, edited_at=?2 WHERE id=?3",
        params![req.content, Utc::now().to_rfc3339(), id.to_string()],
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let msg = db::hydrate_message(&conn, &id.to_string(), &user.id.to_string())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap();
    let _ = state.events.send(WsEvent::MessageEdited { message: msg.clone() });
    Ok(Json(msg))
}

async fn delete_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    let user = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    let row: Option<(String, String)> = conn
        .query_row("SELECT author_id, channel_id FROM messages WHERE id=?1", params![id.to_string()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some((author, channel_id)) = row else {
        return Err(err(StatusCode::NOT_FOUND, "not found"));
    };
    if author != user.id.to_string() {
        return Err(err(StatusCode::FORBIDDEN, "not your message"));
    }
    conn.execute("DELETE FROM messages WHERE id=?1", params![id.to_string()])
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let channel_id: Uuid = channel_id.parse().unwrap_or_else(|_| Uuid::nil());
    let _ = state.events.send(WsEvent::MessageDeleted { channel_id, id });
    Ok(StatusCode::NO_CONTENT)
}

async fn toggle_reaction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, emoji)): Path<(Uuid, String)>,
) -> ApiResult<Json<Message>> {
    let user = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    let row: Option<(String, String)> = conn
        .query_row("SELECT id, channel_id FROM messages WHERE id=?1", params![id.to_string()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some((_, channel_id)) = row else {
        return Err(err(StatusCode::NOT_FOUND, "not found"));
    };
    let exists: Option<String> = conn
        .query_row(
            "SELECT emoji FROM reactions WHERE message_id=?1 AND user_id=?2 AND emoji=?3",
            params![id.to_string(), user.id.to_string(), emoji],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if exists.is_some() {
        conn.execute(
            "DELETE FROM reactions WHERE message_id=?1 AND user_id=?2 AND emoji=?3",
            params![id.to_string(), user.id.to_string(), emoji],
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else {
        conn.execute(
            "INSERT INTO reactions (message_id, user_id, emoji) VALUES (?1,?2,?3)",
            params![id.to_string(), user.id.to_string(), emoji],
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    let msg = db::hydrate_message(&conn, &id.to_string(), &user.id.to_string())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap();
    let _ = state.events.send(WsEvent::ReactionUpdated {
        channel_id: channel_id.parse().unwrap_or_else(|_| Uuid::nil()),
        message_id: id,
        reactions: msg.reactions.clone(),
    });
    Ok(Json(msg))
}

async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SearchRequest>,
) -> ApiResult<Json<Vec<Message>>> {
    let user = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    let like = format!("%{}%", q.query);
    let limit = q.limit.unwrap_or(25).clamp(1, 100) as i64;
    let mut sql = String::from("SELECT id FROM messages WHERE content LIKE ?1");
    let mut args: Vec<String> = vec![like];
    if let Some(cid) = q.channel_id {
        sql.push_str(" AND channel_id=?");
        args.push(cid.to_string());
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ");
    sql.push_str(&limit.to_string());
    let mut stmt = conn.prepare(&sql).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let ids: Vec<String> = if args.len() == 1 {
        stmt.query_map([args[0].clone()], |r| r.get(0))
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        stmt.query_map(params![args[0].clone(), args[1].clone()], |r| r.get::<_, String>(0))
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let mut out = Vec::new();
    for id in ids {
        if let Some(m) = db::hydrate_message(&conn, &id, &user.id.to_string())
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            if let Some(sid) = q.server_id {
                // filter by server via channel lookup
                let chan_server: Option<String> = conn
                    .query_row("SELECT server_id FROM channels WHERE id=?1", params![m.channel_id.to_string()], |r| r.get(0))
                    .optional()
                    .unwrap_or(None);
                if chan_server.as_deref() != Some(&sid.to_string()) {
                    continue;
                }
            }
            if let Some(author) = &q.author {
                if !m.author.name.contains(author) {
                    continue;
                }
            }
            out.push(m);
        }
    }
    Ok(Json(out))
}

async fn list_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(server_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Member>>> {
    let _ = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    let mut stmt = conn
        .prepare("SELECT u.id, u.name, u.display_name, u.bot, u.created_at FROM users u JOIN server_members m ON m.user_id=u.id WHERE m.server_id=?1 ORDER BY u.name")
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rows = stmt
        .query_map(params![server_id.to_string()], |r| {
            Ok(Member {
                user: User {
                    id: r.get::<_, String>(0)?.parse().unwrap_or_else(|_| Uuid::nil()),
                    name: r.get(1)?,
                    display_name: r.get(2)?,
                    avatar_url: None,
                    bot: r.get::<_, i64>(3)? != 0,
                    created_at: r.get::<_, String>(4)?.parse().unwrap_or_else(|_| Utc::now()),
                },
                presence: Presence::Online,
                roles: vec![],
            })
        })
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }
    Ok(Json(out))
}

async fn list_notifications(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Vec<Notification>>> {
    let user = auth_user(&state, &headers).await?;
    let conn = state.db.lock().await;
    let mut stmt = conn
        .prepare("SELECT id, channel_id, message_id, kind, created_at, read FROM notifications WHERE user_id=?1 ORDER BY created_at DESC LIMIT 50")
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rows = stmt
        .query_map(params![user.id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        let (id, channel_id, message_id, kind, created_at, read) =
            r.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some(message) = db::hydrate_message(&conn, &message_id, &user.id.to_string())
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        else {
            continue;
        };
        out.push(Notification {
            id: id.parse().unwrap_or_else(|_| Uuid::nil()),
            channel_id: channel_id.parse().unwrap_or_else(|_| Uuid::nil()),
            message,
            kind: match kind.as_str() {
                "reply" => NotificationKind::Reply,
                "direct" => NotificationKind::Direct,
                _ => NotificationKind::Mention,
            },
            created_at: created_at.parse().unwrap_or_else(|_| Utc::now()),
            read: read != 0,
        });
    }
    Ok(Json(out))
}

// --- websocket ---

async fn ws_handler(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let token = params.get("token").cloned().unwrap_or_default();
    ws.on_upgrade(move |socket| ws_task(state, socket, token))
}

async fn ws_task(state: AppState, socket: WebSocket, token: String) {
    let user_id: Option<String> = {
        let conn = state.db.lock().await;
        conn.query_row("SELECT user_id FROM tokens WHERE token=?1", params![token], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    };
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.events.subscribe();
    if let Some(uid) = user_id {
        let user = {
            let conn = state.db.lock().await;
            db::user_by_id(&conn, &uid).ok().flatten()
        };
        if let Some(user) = user {
            let hello = WsEvent::Hello { protocol: PROTOCOL_VERSION, user };
            if sender
                .send(AxumWsMessage::Text(serde_json::to_string(&hello).unwrap().into()))
                .await
                .is_err()
            {
                return;
            }
        }
    }
    loop {
        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(AxumWsMessage::Text(t))) => {
                        // Accept client commands (typing/subscribe); broadcast typing.
                        if let Ok(WsCommand::Typing { channel_id }) = serde_json::from_str::<WsCommand>(&t) {
                            let _ = channel_id;
                        }
                    }
                    Some(Ok(AxumWsMessage::Close(_))) | None => break,
                    _ => {}
                }
            }
            ev = rx.recv() => {
                match ev {
                    Ok(ev) => {
                        let s = serde_json::to_string(&ev).unwrap();
                        if sender.send(AxumWsMessage::Text(s.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/register", post(register))
        .route("/api/v1/login", post(login))
        .route("/api/v1/me", get(me).patch(update_me))
        .route("/api/v1/me/avatar", post(set_avatar).delete(delete_avatar))
        .route("/api/v1/users/{id}/avatar", get(get_avatar))
        .route("/api/v1/logout", post(logout))
        .route("/api/v1/servers", get(list_servers).post(create_server))
        .route("/api/v1/servers/{id}/channels", get(list_channels).post(create_channel))
        .route("/api/v1/servers/{id}/members", get(list_members))
        .route("/api/v1/channels/{id}/messages", get(channel_history).post(send_message))
        .route("/api/v1/messages/{id}", patch(edit_message).delete(delete_message))
        .route("/api/v1/messages/{id}/reactions/{emoji}", post(toggle_reaction))
        .route("/api/v1/search", get(search))
        .route("/api/v1/notifications", get(list_notifications))
        .route("/api/v1/ws", get(ws_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_router() -> (Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = crate::db::open(&path).unwrap();
        let (tx, _) = tokio::sync::broadcast::channel::<WsEvent>(16);
        let router = router(AppState {
            db: Arc::new(Mutex::new(conn)),
            events: tx,
            allow_registration: true,
            data_dir: dir.path().to_path_buf(),
        });
        (router, dir)
    }

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn json_req(method: &str, uri: &str, token: Option<&str>, body: serde_json::Value) -> Request<axum::body::Body> {
        let mut b = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        b.body(axum::body::Body::from(serde_json::to_vec(&body).unwrap())).unwrap()
    }

    #[tokio::test]
    async fn full_api_smoke() {
        let (app, _dir) = test_router().await;

        // health
        let res = app
            .clone()
            .oneshot(Request::builder().uri("/health").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // register
        let res = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/register",
                None,
                serde_json::json!({"name": "alice", "password": "correct-horse"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let auth = body_json(res).await;
        let token = auth["token"].as_str().unwrap().to_string();

        // me
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/me")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // servers (seeded "general")
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/servers")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let servers = body_json(res).await;
        let server_id = servers[0]["id"].as_str().unwrap().to_string();

        // channels
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/servers/{server_id}/channels"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let channels = body_json(res).await;
        let channel_id = channels[0]["id"].as_str().unwrap().to_string();

        // send
        let res = app
            .clone()
            .oneshot(json_req(
                "POST",
                &format!("/api/v1/channels/{channel_id}/messages"),
                Some(&token),
                serde_json::json!({"content": "hello **world**"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let msg = body_json(res).await;
        let msg_id = msg["id"].as_str().unwrap().to_string();

        // history
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/channels/{channel_id}/messages?limit=10"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // edit
        let res = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                &format!("/api/v1/messages/{msg_id}"),
                Some(&token),
                serde_json::json!({"content": "hello edited"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // react toggle
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/messages/{msg_id}/reactions/%F0%9F%94%A5"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // search
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/search?query=hello")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // update display name
        let res = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/me",
                Some(&token),
                serde_json::json!({"display_name": "Alice Liddell"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let me = body_json(res).await;
        assert_eq!(me["display_name"], "Alice Liddell");

        // upload avatar (1x1 PNG)
        const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        let res = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/me/avatar",
                Some(&token),
                serde_json::json!({"data_base64": TINY_PNG_B64}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let me = body_json(res).await;
        let avatar_path = me["avatar_url"].as_str().unwrap().to_string();
        assert!(avatar_path.ends_with("/avatar"));

        // serve avatar bytes
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(avatar_path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], "image/png");

        // delete avatar
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/me/avatar")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        // delete
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/messages/{msg_id}"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        // logout invalidates the token: /me must now fail.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/logout")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/me")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
