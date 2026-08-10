use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::context::{Client, generate_visitor_data};
use crate::oauth::{self, Tokens};

const API_BASE: &str = "https://www.youtube.com/youtubei/v1/";
const MUSIC_API_BASE: &str = "https://music.youtube.com/youtubei/v1/";

pub struct YtMusic {
    pub(crate) http: reqwest::Client,
    visitor: String,
    tokens: Option<RwLock<Tokens>>,
    cookies: Option<String>,
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

    pub fn with_cookies(cookies: impl Into<String>) -> Self {
        Self {
            cookies: Some(cookies.into()),
            ..Self::anonymous()
        }
    }

    pub fn anonymous() -> Self {
        Self {
            http: reqwest::Client::new(),
            visitor: generate_visitor_data(),
            tokens: None,
            cookies: None,
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
        self.execute_with(endpoint, client, payload, true).await
    }

    pub async fn execute_with(
        &self,
        endpoint: &str,
        client: Client,
        payload: Value,
        use_auth: bool,
    ) -> Result<Value> {
        let bearer = match use_auth {
            true => self.bearer().await?,
            false => None,
        };
        let cookies = self.cookies.as_ref().filter(|_| use_auth);
        let authenticated = bearer.is_some() || cookies.is_some();
        let visitor = match authenticated {
            true => "",
            false => self.visitor.as_str(),
        };
        let mut body = payload;
        let context = client.context(visitor, &self.hl, &self.gl);
        body.as_object_mut()
            .context("payload must be an object")?
            .insert("context".to_string(), context);
        if client == Client::Music {
            body["isAudioOnly"] = json!(true);
        }
        let (base, origin) = match client {
            Client::Music => (MUSIC_API_BASE, "https://music.youtube.com"),
            _ => (API_BASE, "https://www.youtube.com"),
        };
        let url = format!("{base}{endpoint}?prettyPrint=false&alt=json");
        let mut request = self
            .http
            .post(&url)
            .header("Accept", "*/*")
            .header("Accept-Language", "*")
            .header("Content-Type", "application/json")
            .header("Origin", origin)
            .header("User-Agent", client.user_agent())
            .header("X-Youtube-Client-Name", client.id().to_string())
            .header("X-Youtube-Client-Version", client.version())
            .json(&body);
        match (cookies, &bearer) {
            (Some(cookies), _) => {
                let sapisid = sapisid(cookies).context("cookies have no SAPISID")?;
                request = request
                    .header("Authorization", sid_authorization(sapisid, origin))
                    .header("Cookie", cookies)
                    .header("X-Origin", origin)
                    .header("X-Goog-AuthUser", "0");
            }
            (None, Some(bearer)) => {
                request = request.header("Authorization", format!("Bearer {bearer}"));
            }
            (None, None) => request = request.header("X-Goog-Visitor-Id", &self.visitor),
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("cannot reach {endpoint}"))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .with_context(|| format!("cannot read {endpoint} response"))?;
        log::debug!(
            "{endpoint} via {}: {status}, {} bytes",
            client.name(),
            body.len()
        );
        let Ok(value) = serde_json::from_slice::<Value>(&body) else {
            log::warn!(
                "{endpoint} non-json body: {}",
                String::from_utf8_lossy(&body[..body.len().min(600)])
            );
            bail!("{endpoint} returned non-json response with status {status}");
        };
        if let Some(error) = value.get("error") {
            log::warn!("{endpoint} error body: {error}");
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

    pub fn is_cookie_auth(&self) -> bool {
        self.cookies.is_some()
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

fn sapisid(cookies: &str) -> Option<&str> {
    ["SAPISID=", "__Secure-3PAPISID="].iter().find_map(|key| {
        cookies.split(';').find_map(|pair| {
            let pair = pair.trim();
            pair.strip_prefix(key)
        })
    })
}

fn sid_authorization(sapisid: &str, origin: &str) -> String {
    use sha1::{Digest as _, Sha1};
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let mut hasher = Sha1::new();
    hasher.update(format!("{timestamp} {sapisid} {origin}"));
    let hash = hasher.finalize();
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("SAPISIDHASH {timestamp}_{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_sapisid() {
        let cookies = "VISITOR_INFO1_LIVE=abc; SAPISID=xyz/123; __Secure-3PAPISID=xyz/123";
        assert_eq!(sapisid(cookies), Some("xyz/123"));
    }

    #[test]
    fn falls_back_to_secure_sapisid() {
        let cookies = "__Secure-3PAPISID=only/456";
        assert_eq!(sapisid(cookies), Some("only/456"));
    }

    #[test]
    fn sid_hash_shape() {
        let auth = sid_authorization("abc", "https://music.youtube.com");
        assert!(auth.starts_with("SAPISIDHASH "));
        assert_eq!(auth.split('_').nth(1).map(str::len), Some(40));
    }
}
