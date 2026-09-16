use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use tuimessager_protocol::*;
use uuid::Uuid;

pub fn open(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).context("create data dir")?;
        }
    }
    let conn = Connection::open(path).context("open sqlite")?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    init_schema(&conn)?;
    seed_if_empty(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            display_name TEXT,
            password_hash TEXT NOT NULL,
            bot INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tokens (
            token TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS servers (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS server_members (
            server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            PRIMARY KEY (server_id, user_id)
        );
        CREATE TABLE IF NOT EXISTS channels (
            id TEXT PRIMARY KEY,
            server_id TEXT REFERENCES servers(id) ON DELETE CASCADE,
            parent_id TEXT REFERENCES channels(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            topic TEXT,
            kind TEXT NOT NULL DEFAULT 'text',
            position INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
            author_id TEXT NOT NULL REFERENCES users(id),
            content TEXT NOT NULL,
            created_at TEXT NOT NULL,
            edited_at TEXT,
            reply_to TEXT,
            pinned INTEGER NOT NULL DEFAULT 0,
            nonce TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel_id, created_at);
        CREATE TABLE IF NOT EXISTS reactions (
            message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            emoji TEXT NOT NULL,
            PRIMARY KEY (message_id, user_id, emoji)
        );
        CREATE TABLE IF NOT EXISTS notifications (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            channel_id TEXT NOT NULL,
            message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            created_at TEXT NOT NULL,
            read INTEGER NOT NULL DEFAULT 0
        );
        "#,
    )?;
    Ok(())
}

fn seed_if_empty(conn: &Connection) -> Result<()> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM servers", [], |r| r.get(0))?;
    if count > 0 {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    let server_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO servers (id, name, description, created_at) VALUES (?1,'general','Welcome to tuimessager — your self-hosted Concord-style chat',?2)",
        params![server_id, now],
    )?;
    for (i, (name, topic)) in [("general", Some("General discussion")), ("random", Some("Off-topic"))].iter().enumerate() {
        let cid = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO channels (id, server_id, name, topic, kind, position) VALUES (?1,?2,?3,?4,'text',?5)",
            params![cid, server_id, name, topic, i as i64],
        )?;
    }
    Ok(())
}

pub fn user_by_id(conn: &Connection, id: &str) -> Result<Option<User>> {
    conn.query_row(
        "SELECT id, name, display_name, bot, created_at FROM users WHERE id=?1",
        params![id],
        map_user,
    )
    .optional()
    .map_err(anyhow::Error::from)
}

pub fn user_by_name(conn: &Connection, name: &str) -> Result<Option<(User, String)>> {
    conn.query_row(
        "SELECT id, name, display_name, bot, created_at, password_hash FROM users WHERE name=?1",
        params![name],
        |r| {
            Ok((
                User {
                    id: r.get::<_, String>(0)?.parse().map_err(|_| rusqlite::Error::InvalidColumnType(0, "uuid".into(), rusqlite::types::Type::Text))?,
                    name: r.get(1)?,
                    display_name: r.get(2)?,
                    avatar_url: None,
                    bot: r.get::<_, i64>(3)? != 0,
                    created_at: r.get::<_, String>(4)?.parse().unwrap_or_else(|_| Utc::now()),
                },
                r.get::<_, String>(5)?,
            ))
        },
    )
    .optional()
    .map_err(anyhow::Error::from)
}

fn map_user(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get::<_, String>(0)?.parse().unwrap_or_else(|_| Uuid::nil()),
        name: r.get(1)?,
        display_name: r.get(2)?,
        avatar_url: None,
        bot: r.get::<_, i64>(3)? != 0,
        created_at: r.get::<_, String>(4)?.parse().unwrap_or_else(|_| chrono::Utc::now()),
    })
}

pub fn message_reactions(conn: &Connection, message_id: &str, me: &str) -> Result<Vec<ReactionCount>> {
    let mut stmt = conn.prepare(
        "SELECT emoji, COUNT(*), SUM(CASE WHEN user_id=?2 THEN 1 ELSE 0 END) FROM reactions WHERE message_id=?1 GROUP BY emoji",
    )?;
    let rows = stmt.query_map(params![message_id, me], |r| {
        let emoji: String = r.get(0)?;
        let count: i64 = r.get(1)?;
        let mine: Option<i64> = r.get(2)?;
        Ok(ReactionCount {
            emoji,
            count: count as u64,
            me: mine.unwrap_or(0) > 0,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn hydrate_message(conn: &Connection, message_id: &str, me: &str) -> Result<Option<Message>> {
    let row: Option<(String, String, String, String, String, Option<String>, Option<String>, i64)> = conn
        .query_row(
            "SELECT id, channel_id, author_id, content, created_at, edited_at, reply_to, pinned FROM messages WHERE id=?1",
            params![message_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .optional()?;
    let Some((id, channel_id, author_id, content, created_at, edited_at, reply_to, pinned)) = row else {
        return Ok(None);
    };
    let author = user_by_id(conn, &author_id)?.context("author missing")?;
    Ok(Some(Message {
        id: id.parse().unwrap_or_else(|_| Uuid::nil()),
        channel_id: channel_id.parse().unwrap_or_else(|_| Uuid::nil()),
        author,
        content,
        created_at: created_at.parse().unwrap_or_else(|_| Utc::now()),
        edited_at: edited_at.and_then(|s: String| s.parse().ok()),
        reply_to: reply_to.and_then(|s| s.parse().ok()),
        pinned: pinned != 0,
        reactions: message_reactions(conn, &id, me)?,
        attachments: vec![],
        embeds: vec![],
    }))
}
