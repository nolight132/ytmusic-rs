use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::random_string;

const CODE_URL: &str = "https://www.youtube.com/o/oauth2/device/code";
const TOKEN_URL: &str = "https://www.youtube.com/o/oauth2/token";
const REVOKE_URL: &str = "https://www.youtube.com/o/oauth2/revoke";
const TV_URL: &str = "https://www.youtube.com/tv";
const TV_UA: &str = "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version";
const SCOPE: &str = "http://gdata.youtube.com https://www.googleapis.com/auth/youtube-paid-content";
const DEVICE_GRANT: &str = "http://oauth.net/grant_type/device/1.0";
const REFRESH_MARGIN: u64 = 60;

const FALLBACK_ID: &str =
    "861556708454-d6dlm3lh05idd8npek18k6be8ba3oc68.apps.googleusercontent.com";
const FALLBACK_SECRET: &str = "SboVhoG9s0rNafixCSGGKXAT";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientIdentity {
    pub id: String,
    pub secret: String,
}

impl ClientIdentity {
    pub fn fallback() -> Self {
        Self {
            id: FALLBACK_ID.to_string(),
            secret: FALLBACK_SECRET.to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub client: ClientIdentity,
}

impl Tokens {
    pub fn expired(&self) -> bool {
        now() + REFRESH_MARGIN >= self.expires_at
    }

    pub fn load(path: &Path) -> Result<Option<Self>> {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("cannot read token cache"),
        };
        let tokens = serde_json::from_slice(&data).context("cannot parse token cache")?;
        Ok(Some(tokens))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("cannot create token cache dir")?;
        }
        let data = serde_json::to_vec_pretty(self).context("cannot serialize tokens")?;
        std::fs::write(path, data).context("cannot write token cache")?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval: Duration,
    pub expires_in: Duration,
}

pub async fn fetch_identity(http: &reqwest::Client) -> ClientIdentity {
    match scrape_identity(http).await {
        Ok(identity) => identity,
        Err(error) => {
            log::warn!("oauth: identity scrape failed ({error:#}), using fallback");
            ClientIdentity::fallback()
        }
    }
}

pub async fn request_device_code(
    http: &reqwest::Client,
    identity: &ClientIdentity,
) -> Result<DeviceCode> {
    let device_id = format!(
        "{}-{}-{}-{}-{}",
        random_string(8),
        random_string(4),
        random_string(4),
        random_string(4),
        random_string(12)
    );
    let body = json!({
        "client_id": identity.id,
        "scope": SCOPE,
        "device_id": device_id,
        "device_model": "ytlr::",
    });
    let response: Value = http
        .post(CODE_URL)
        .json(&body)
        .send()
        .await
        .context("cannot reach oauth device endpoint")?
        .json()
        .await
        .context("cannot parse device code response")?;
    if let Some(error) = response.get("error_code").and_then(Value::as_str) {
        bail!("device code request failed: {error}");
    }
    let field = |key: &str| -> Result<String> {
        response
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .with_context(|| format!("device code response missing {key}"))
    };
    Ok(DeviceCode {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        verification_url: field("verification_url")?,
        interval: Duration::from_secs(response["interval"].as_u64().unwrap_or(5)),
        expires_in: Duration::from_secs(response["expires_in"].as_u64().unwrap_or(1800)),
    })
}

pub async fn poll_token(
    http: &reqwest::Client,
    identity: &ClientIdentity,
    device: &DeviceCode,
) -> Result<Tokens> {
    let deadline = SystemTime::now() + device.expires_in;
    let mut interval = device.interval;
    loop {
        if SystemTime::now() > deadline {
            bail!("sign-in code expired");
        }
        tokio::time::sleep(interval).await;
        let body = json!({
            "client_id": identity.id,
            "client_secret": identity.secret,
            "code": device.device_code,
            "grant_type": DEVICE_GRANT,
        });
        let response: Value = http
            .post(TOKEN_URL)
            .json(&body)
            .send()
            .await
            .context("cannot reach oauth token endpoint")?
            .json()
            .await
            .context("cannot parse token response")?;
        match response.get("error").and_then(Value::as_str) {
            None => return tokens_from(response, identity.clone()),
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => bail!("sign-in code expired"),
            Some("access_denied") => bail!("sign-in was denied"),
            Some(error) => bail!("oauth token request failed: {error}"),
        }
    }
}

pub async fn refresh(http: &reqwest::Client, tokens: &mut Tokens) -> Result<()> {
    let body = json!({
        "client_id": tokens.client.id,
        "client_secret": tokens.client.secret,
        "refresh_token": tokens.refresh_token,
        "grant_type": "refresh_token",
    });
    let response: Value = http
        .post(TOKEN_URL)
        .json(&body)
        .send()
        .await
        .context("cannot reach oauth token endpoint")?
        .json()
        .await
        .context("cannot parse refresh response")?;
    if let Some(error) = response.get("error").and_then(Value::as_str) {
        bail!("token refresh failed: {error}");
    }
    let access = response
        .get("access_token")
        .and_then(Value::as_str)
        .context("refresh response missing access_token")?;
    tokens.access_token = access.to_string();
    tokens.expires_at = now() + response["expires_in"].as_u64().unwrap_or(3600);
    Ok(())
}

pub async fn revoke(http: &reqwest::Client, tokens: &Tokens) -> Result<()> {
    http.post(REVOKE_URL)
        .query(&[("token", tokens.access_token.as_str())])
        .send()
        .await
        .context("cannot reach oauth revoke endpoint")?;
    Ok(())
}

fn tokens_from(response: Value, client: ClientIdentity) -> Result<Tokens> {
    let access = response
        .get("access_token")
        .and_then(Value::as_str)
        .context("token response missing access_token")?;
    let refresh = response
        .get("refresh_token")
        .and_then(Value::as_str)
        .context("token response missing refresh_token")?;
    Ok(Tokens {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at: now() + response["expires_in"].as_u64().unwrap_or(3600),
        client,
    })
}

async fn scrape_identity(http: &reqwest::Client) -> Result<ClientIdentity> {
    let page = http
        .get(TV_URL)
        .header("User-Agent", TV_UA)
        .header("Referer", TV_URL)
        .send()
        .await?
        .text()
        .await?;
    let script = regex_lite::Regex::new(r#"<script\s+id="base-js"\s+src="([^"]+)""#)?
        .captures(&page)
        .and_then(|captures| captures.get(1))
        .context("cannot find base-js script")?
        .as_str()
        .to_string();
    let url = match script.starts_with("http") {
        true => script,
        false => format!("https://www.youtube.com{script}"),
    };
    let body = http
        .get(url)
        .header("User-Agent", TV_UA)
        .send()
        .await?
        .text()
        .await?;
    let captures = regex_lite::Regex::new(r#"clientId:"([^"]+)",[^"]*?:"([^"]+)""#)?
        .captures(&body)
        .context("cannot find client identity in base-js")?;
    Ok(ClientIdentity {
        id: captures[1].to_string(),
        secret: captures[2].to_string(),
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
