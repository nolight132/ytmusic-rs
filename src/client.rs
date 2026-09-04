use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::context::Client;
use crate::oauth::{self, Tokens};

const API_BASE: &str = "https://www.youtube.com/youtubei/v1/";
const MUSIC_API_BASE: &str = "https://music.youtube.com/youtubei/v1/";
const VISITOR_URL: &str = "https://www.youtube.com/sw.js_data";

pub struct YtMusic {
    pub(crate) http: reqwest::Client,
    visitor: RwLock<Option<String>>,
    solver: RwLock<Option<std::sync::Arc<crate::deobf::Solver>>>,
    player_cache: Option<PathBuf>,
    tokens: Option<RwLock<Tokens>>,
    cookies: Option<String>,
    authuser: usize,
    page_id: Option<String>,
    persist: Option<PathBuf>,
    pub(crate) resolve_cache: crate::dedup::ResolveCache,
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
            cookies: Some(normalize_cookies(&cookies.into())),
            ..Self::anonymous()
        }
    }

    pub fn anonymous() -> Self {
        Self {
            http: reqwest::Client::new(),
            visitor: RwLock::new(None),
            solver: RwLock::new(None),
            player_cache: None,
            tokens: None,
            cookies: None,
            authuser: 0,
            page_id: None,
            persist: None,
            resolve_cache: crate::dedup::ResolveCache::memory(),
            hl: "en".to_string(),
            gl: "US".to_string(),
        }
    }

    pub fn as_user(mut self, authuser: usize) -> Self {
        self.authuser = authuser;
        self
    }

    /// Send authenticated requests on behalf of a YouTube Brand Account.
    ///
    /// `page_id` is the numeric Brand Account identifier shown in the Google
    /// account URL after selecting the brand (for example,
    /// `myaccount.google.com/b/<page_id>`).
    pub fn as_brand_account(mut self, page_id: impl Into<String>) -> Self {
        self.page_id = Some(page_id.into());
        self
    }

    pub fn persist_to(mut self, path: PathBuf) -> Self {
        self.persist = Some(path);
        self
    }

    pub fn cache_resolutions(mut self, path: PathBuf) -> Self {
        self.resolve_cache = crate::dedup::ResolveCache::disk(path);
        self
    }

    pub fn cache_player(mut self, path: PathBuf) -> Self {
        self.player_cache = Some(path);
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
        self.execute_visiting(endpoint, client, payload, use_auth, None)
            .await
    }

    pub(crate) async fn execute_visiting(
        &self,
        endpoint: &str,
        client: Client,
        payload: Value,
        use_auth: bool,
        guest: Option<&str>,
    ) -> Result<Value> {
        let bearer = match use_auth {
            true => self.bearer().await?,
            false => None,
        };
        let cookies = self.cookies.as_ref().filter(|_| use_auth);
        let authenticated = bearer.is_some() || cookies.is_some();
        let held = match authenticated {
            true => String::new(),
            false => match guest {
                Some(guest) => guest.to_string(),
                None => self.visitor().await,
            },
        };
        let visitor = held.as_str();
        let mut body = payload;
        let context = self.context(client, visitor);
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
                let authorization =
                    sid_authorization(cookies, origin).context("cookies have no SAPISID")?;
                request = request
                    .header("Authorization", authorization)
                    .header("Cookie", cookies)
                    .header("X-Origin", origin)
                    .header("X-Goog-AuthUser", self.authuser.to_string());
                if let Some(page_id) = &self.page_id {
                    request = request.header("X-Goog-PageId", page_id);
                }
            }
            (None, Some(bearer)) => {
                request = request.header("Authorization", format!("Bearer {bearer}"));
            }
            (None, None) => request = request.header("X-Goog-Visitor-Id", visitor),
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

    pub fn is_authenticated(&self) -> bool {
        self.cookies.is_some() || self.tokens.is_some()
    }

    pub fn authuser(&self) -> usize {
        self.authuser
    }

    fn context(&self, client: Client, visitor: &str) -> Value {
        let mut context = client.context(visitor, &self.hl, &self.gl);
        if let Some(page_id) = &self.page_id {
            context["user"]["onBehalfOfUser"] = json!(page_id);
        }
        context
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) async fn solver(&self) -> Result<std::sync::Arc<crate::deobf::Solver>> {
        if let Some(ready) = self.solver.read().await.clone() {
            return Ok(ready);
        }
        let mut slot = self.solver.write().await;
        if let Some(ready) = slot.clone() {
            return Ok(ready);
        }
        let cache = self.player_cache.clone();
        let script = crate::deobf::fetch(&self.http, cache.as_deref()).await?;
        let id = script.id.clone();
        let started = std::time::Instant::now();
        let solver =
            tokio::task::spawn_blocking(move || crate::deobf::Solver::start(script, cache))
                .await
                .context("the deobfuscator did not start")??;
        log::debug!("deobf: player {id} ready in {:?}", started.elapsed());
        let solver = std::sync::Arc::new(solver);
        *slot = Some(solver.clone());
        Ok(solver)
    }

    pub(crate) async fn visitor(&self) -> String {
        if let Some(ready) = self.visitor.read().await.clone() {
            return ready;
        }
        let mut slot = self.visitor.write().await;
        if let Some(ready) = slot.clone() {
            return ready;
        }
        match fetch_visitor(&self.http).await {
            Ok(issued) => {
                *slot = Some(issued.clone());
                issued
            }
            Err(error) => {
                log::warn!("ytmusic: cannot fetch a visitor id, asking again next time: {error:#}");
                String::new()
            }
        }
    }

    pub(crate) async fn adopt_visitor(&self, issued: String) {
        *self.visitor.write().await = Some(issued);
    }

    pub fn lang(&self) -> &str {
        &self.hl
    }

    pub fn region(&self) -> &str {
        &self.gl
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

async fn fetch_visitor(http: &reqwest::Client) -> Result<String> {
    let body = http
        .get(VISITOR_URL)
        .header("User-Agent", Client::VisionOs.user_agent())
        .send()
        .await
        .context("cannot reach the visitor endpoint")?
        .text()
        .await
        .context("cannot read the visitor response")?;
    let issued = regex_lite::Regex::new(r#""(Cg[A-Za-z0-9%_+=-]{20,})""#)?
        .captures(&body)
        .and_then(|found| found.get(1))
        .map(|found| found.as_str().to_string())
        .context("the visitor response carries no visitor id")?;
    log::debug!("ytmusic: adopted a server-issued visitor id");
    Ok(issued)
}

fn normalize_cookies(input: &str) -> String {
    let raw = input
        .lines()
        .find_map(|line| {
            let line = line.trim();
            let rest = line
                .strip_prefix("Cookie:")
                .or_else(|| line.strip_prefix("cookie:"))?;
            Some(rest.trim())
        })
        .unwrap_or_else(|| input.trim());
    let pairs: Vec<&str> = raw
        .split(';')
        .map(str::trim)
        .filter(|pair| pair.contains('=') && !pair.contains(char::is_whitespace))
        .collect();
    pairs.join("; ")
}

fn cookie_value<'a>(cookies: &'a str, name: &str) -> Option<&'a str> {
    cookies.split(';').find_map(|pair| {
        let pair = pair.trim();
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

fn sapisid(cookies: &str) -> Option<&str> {
    cookie_value(cookies, "SAPISID").or_else(|| cookie_value(cookies, "__Secure-3PAPISID"))
}

fn sid_authorization(cookies: &str, origin: &str) -> Option<String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let sapisid = sapisid(cookies)?;
    Some(format!(
        "SAPISIDHASH {}",
        sid_hash(timestamp, sapisid, origin)
    ))
}

fn sid_hash(timestamp: u64, secret: &str, origin: &str) -> String {
    use sha1::{Digest as _, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(format!("{timestamp} {secret} {origin}"));
    let hash = hasher.finalize();
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{timestamp}_{hex}")
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
    fn normalizes_header_blob() {
        let blob = "POST /youtubei/v1/browse HTTP/2\nHost: music.youtube.com\nCookie: SAPISID=abc; SID=def\nOrigin: https://music.youtube.com";
        assert_eq!(normalize_cookies(blob), "SAPISID=abc; SID=def");
    }

    #[test]
    fn normalizes_plain_cookie_string() {
        let plain = "SAPISID=abc;   SID=def  ";
        assert_eq!(normalize_cookies(plain), "SAPISID=abc; SID=def");
    }

    #[test]
    fn falls_back_to_secure_sapisid() {
        let cookies = "__Secure-3PAPISID=only/456";
        assert_eq!(sapisid(cookies), Some("only/456"));
    }

    #[test]
    fn brand_account_is_added_to_the_request_context() {
        let client = YtMusic::anonymous().as_brand_account("101234161234936123473");
        let context = client.context(Client::Music, "visitor");
        assert_eq!(context["user"]["onBehalfOfUser"], "101234161234936123473");
    }

    #[test]
    fn sid_hash_shape() {
        let cookies = "SAPISID=abc; __Secure-3PAPISID=ghi";
        let auth = sid_authorization(cookies, "https://music.youtube.com").unwrap();
        assert!(auth.starts_with("SAPISIDHASH "));
        assert_eq!(auth.split('_').nth(1).map(str::len), Some(40));
    }
}
