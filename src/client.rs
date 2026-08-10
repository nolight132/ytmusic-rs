use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::context::{Client, generate_visitor_data};
use crate::oauth::{self, Tokens};

const API_BASE: &str = "https://www.youtube.com/youtubei/v1/";

pub struct YtMusic {
    pub(crate) http: reqwest::Client,
    visitor: String,
    tokens: Option<RwLock<Tokens>>,
    persist: Option<PathBuf>,
    hl: String,
    gl: String,
}

impl YtMusic {
    pub fn new(tokens: Tokens) -> Self {
        Self {
            tokens: Some(RwLock::new(tokens)),
            ..Self::anonymous()
        }
    }

    pub fn anonymous() -> Self {
        Self {
            http: reqwest::Client::new(),
            visitor: generate_visitor_data(),
            tokens: None,
            persist: None,
            hl: "en".to_string(),
            gl: "US".to_string(),
        }
    }

    pub fn persist_to(mut self, path: PathBuf) -> Self {
        self.persist = Some(path);
        self
    }

    pub async fn tokens(&self) -> Option<Tokens> {
        match &self.tokens {
            Some(tokens) => Some(tokens.read().await.clone()),
            None => None,
        }
    }

    pub async fn revoke(&self) -> Result<()> {
        let Some(tokens) = self.tokens().await else {
            return Ok(());
        };
        oauth::revoke(&self.http, &tokens).await
    }

    pub async fn execute(&self, endpoint: &str, client: Client, payload: Value) -> Result<Value> {
        let bearer = self.bearer().await?;
        let mut body = payload;
        let context = client.context(&self.visitor, &self.hl, &self.gl);
        body.as_object_mut()
            .context("payload must be an object")?
            .insert("context".to_string(), context);
        if client == Client::Music {
            body["isAudioOnly"] = json!(true);
        }
        let url = format!("{API_BASE}{endpoint}?prettyPrint=false&alt=json");
        let mut request = self
            .http
            .post(&url)
            .header("Accept", "*/*")
            .header("Accept-Language", "*")
            .header("Content-Type", "application/json")
            .header("Origin", "https://www.youtube.com")
            .header("User-Agent", client.user_agent())
            .header("X-Goog-Visitor-Id", &self.visitor)
            .header("X-Youtube-Client-Name", client.id().to_string())
            .header("X-Youtube-Client-Version", client.version())
            .json(&body);
        if let Some(bearer) = bearer {
            request = request.header("Authorization", format!("Bearer {bearer}"));
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("cannot reach {endpoint}"))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .with_context(|| format!("cannot parse {endpoint} response"))?;
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("{endpoint} failed ({status}): {message}");
        }
        if !status.is_success() {
            bail!("{endpoint} failed with status {status}");
        }
        Ok(value)
    }

    async fn bearer(&self) -> Result<Option<String>> {
        let Some(slot) = &self.tokens else {
            return Ok(None);
        };
        {
            let tokens = slot.read().await;
            if !tokens.expired() {
                return Ok(Some(tokens.access_token.clone()));
            }
        }
        let mut tokens = slot.write().await;
        if tokens.expired() {
            oauth::refresh(&self.http, &mut tokens).await?;
            if let Some(path) = &self.persist
                && let Err(error) = tokens.save(path)
            {
                log::warn!("ytmusic: cannot persist refreshed tokens: {error:#}");
            }
        }
        Ok(Some(tokens.access_token.clone()))
    }
}
