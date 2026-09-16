//! REST + WebSocket client for tuimessager-server. Replaces Concord's
//! Discord `client/rest/gateway` modules with a small self-hosted API.

use tuimessager_protocol::*;
use uuid::Uuid;

#[derive(Clone)]
pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token: token.into(),
            http: reqwest::Client::new(),
        }
    }

    pub fn ws_url(&self) -> String {
        let http = self.base.clone();
        let ws = http
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1);
        format!("{ws}/api/v1/ws?token={}", self.token)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let res = self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        check(res).await?.json().await.map_err(anyhow::Error::from)
    }

    async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        let res = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;
        check(res).await?.json().await.map_err(anyhow::Error::from)
    }

    pub async fn me(&self) -> anyhow::Result<User> {
        self.get("/api/v1/me").await
    }

    pub async fn servers(&self) -> anyhow::Result<Vec<Server>> {
        self.get("/api/v1/servers").await
    }

    pub async fn channels(&self, server: Uuid) -> anyhow::Result<Vec<Channel>> {
        self.get(&format!("/api/v1/servers/{server}/channels")).await
    }

    pub async fn members(&self, server: Uuid) -> anyhow::Result<Vec<Member>> {
        self.get(&format!("/api/v1/servers/{server}/members")).await
    }

    pub async fn history(&self, channel: Uuid, before: Option<Uuid>, limit: i64) -> anyhow::Result<MessagesPage> {
        let mut url = format!("/api/v1/channels/{channel}/messages?limit={limit}");
        if let Some(b) = before {
            url.push_str(&format!("&before={b}"));
        }
        self.get(&url).await
    }

    pub async fn send(&self, channel: Uuid, content: String, reply_to: Option<Uuid>) -> anyhow::Result<Message> {
        self.post(
            &format!("/api/v1/channels/{channel}/messages"),
            &SendMessageRequest { content, reply_to, nonce: Some(Uuid::new_v4()) },
        )
        .await
    }

    pub async fn edit(&self, id: Uuid, content: String) -> anyhow::Result<Message> {
        let res = self
            .http
            .patch(format!("{}/api/v1/messages/{id}", self.base))
            .bearer_auth(&self.token)
            .json(&EditMessageRequest { content })
            .send()
            .await?;
        check(res).await?.json().await.map_err(anyhow::Error::from)
    }

    pub async fn delete(&self, id: Uuid) -> anyhow::Result<()> {
        let res = self
            .http
            .delete(format!("{}/api/v1/messages/{id}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        check(res).await?;
        Ok(())
    }

    pub async fn react(&self, id: Uuid, emoji: &str) -> anyhow::Result<Message> {
        let res = self
            .http
            .post(format!("{}/api/v1/messages/{id}/reactions/{emoji}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        check(res).await?.json().await.map_err(anyhow::Error::from)
    }

    pub async fn search(&self, query: &str, channel: Option<Uuid>) -> anyhow::Result<Vec<Message>> {
        let mut url = format!("/api/v1/search?query={}", urlencoding(query));
        if let Some(c) = channel {
            url.push_str(&format!("&channel_id={c}"));
        }
        self.get(&url).await
    }

    pub async fn notifications(&self) -> anyhow::Result<Vec<Notification>> {
        self.get("/api/v1/notifications").await
    }

    pub async fn logout(&self) -> anyhow::Result<()> {
        let res = self
            .http
            .post(format!("{}/api/v1/logout", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        check(res).await?;
        Ok(())
    }

    pub async fn update_me(&self, display_name: Option<String>, password: Option<String>) -> anyhow::Result<User> {
        let res = self
            .http
            .patch(format!("{}/api/v1/me", self.base))
            .bearer_auth(&self.token)
            .json(&UpdateMeRequest { display_name, password })
            .send()
            .await?;
        check(res).await?.json().await.map_err(anyhow::Error::from)
    }

    pub async fn set_avatar_path(&self, path: &str) -> anyhow::Result<User> {
        let bytes = std::fs::read(path)?;
        if bytes.is_empty() || bytes.len() > 1_000_000 {
            anyhow::bail!("image must be 1 byte–1 MiB");
        }
        use base64::Engine;
        let data_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let res = self
            .http
            .post(format!("{}/api/v1/me/avatar", self.base))
            .bearer_auth(&self.token)
            .json(&SetAvatarRequest { data_base64 })
            .send()
            .await?;
        check(res).await?.json().await.map_err(anyhow::Error::from)
    }

    pub async fn delete_avatar(&self) -> anyhow::Result<()> {
        let res = self
            .http
            .delete(format!("{}/api/v1/me/avatar", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        check(res).await?;
        Ok(())
    }

    pub async fn login(base: &str, name: &str, password: &str) -> anyhow::Result<AuthResponse> {
        let res = reqwest::Client::new()
            .post(format!("{}/api/v1/login", base.trim_end_matches('/')))
            .json(&LoginRequest { name: name.to_string(), password: password.to_string() })
            .send()
            .await?;
        check(res).await?.json().await.map_err(anyhow::Error::from)
    }

    pub async fn register(base: &str, name: &str, password: &str) -> anyhow::Result<AuthResponse> {
        let res = reqwest::Client::new()
            .post(format!("{}/api/v1/register", base.trim_end_matches('/')))
            .json(&RegisterRequest { name: name.to_string(), display_name: None, password: password.to_string() })
            .send()
            .await?;
        check(res).await?.json().await.map_err(anyhow::Error::from)
    }
}

async fn check(res: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    if res.status().is_success() {
        Ok(res)
    } else {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("{status}: {body}")
    }
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
