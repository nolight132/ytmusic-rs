use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::client::YtMusic;
use crate::context::{Client, random_string};
use crate::models::AudioFormat;

const CHUNK: u64 = 1024 * 1024;
const PARALLEL: usize = 4;
const MAX_STREAM_BYTES: u64 = 256 * 1024 * 1024;
const PLAYER_URL: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false&alt=json";
const VR_UA: &str = "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";

#[derive(Clone, Debug)]
pub struct Playability {
    pub status: String,
    pub reason: Option<String>,
}

impl Playability {
    pub fn ok(&self) -> bool {
        self.status == "OK"
    }
}

impl YtMusic {
    pub async fn player_response(&self, video_id: &str, client: Client) -> Result<Value> {
        let payload = json!({
            "videoId": video_id,
            "racyCheckOk": true,
            "contentCheckOk": true,
            "cpn": random_string(16),
            "playbackContext": {
                "contentPlaybackContext": {
                    "vis": 0,
                    "splay": false,
                    "lactMilliseconds": "-1",
                }
            },
        });
        self.execute_with("player", client, payload, !self.is_cookie_auth())
            .await
    }

    pub async fn best_audio(&self, video_id: &str) -> Result<AudioFormat> {
        match self.direct_audio(video_id).await {
            Ok(format) => Ok(format),
            Err(direct) => {
                log::debug!("player: no direct stream for {video_id} ({direct:#}), deciphering");
                self.deciphered_audio(video_id)
                    .await
                    .with_context(|| format!("no direct stream either ({direct:#})"))
            }
        }
    }

    pub async fn load_audio(&self, video_id: &str) -> Result<(AudioFormat, Vec<u8>)> {
        let started = std::time::Instant::now();
        log::debug!("player: loading {video_id}, trying the direct stream first");
        let refused = match self.direct_audio(video_id).await {
            Ok(format) => match self.download(&format).await {
                Ok(data) => {
                    log::debug!(
                        "player: {video_id} loaded direct, {} in {:?}",
                        describe(&format, data.len()),
                        started.elapsed()
                    );
                    return Ok((format, data));
                }
                Err(refused) => refused,
            },
            Err(missing) => missing,
        };
        log::debug!("player: the direct stream for {video_id} failed ({refused:#}), deciphering");
        let format = self
            .deciphered_audio(video_id)
            .await
            .with_context(|| format!("the direct stream failed too ({refused:#})"))?;
        let data = self.download(&format).await?;
        log::debug!(
            "player: {video_id} loaded deciphered, {} in {:?}",
            describe(&format, data.len()),
            started.elapsed()
        );
        Ok((format, data))
    }

    pub async fn direct_audio(&self, video_id: &str) -> Result<AudioFormat> {
        let response = self.stream_player(video_id).await?;
        let playability = playability(&response);
        if !playability.ok() {
            bail!(
                "{} is not playable: {} ({})",
                video_id,
                playability.status,
                playability.reason.as_deref().unwrap_or("no reason")
            );
        }
        let formats = response
            .get("streamingData")
            .and_then(|data| data.get("adaptiveFormats"))
            .and_then(Value::as_array)
            .context("player response has no adaptive formats")?;
        let mut audio: Vec<AudioFormat> = formats
            .iter()
            .filter_map(|format| audio_format(format, Client::AndroidVr))
            .collect();
        audio.sort_by_key(|format| std::cmp::Reverse(format.bitrate));
        pick(audio).with_context(|| format!("no direct audio stream for {video_id}"))
    }

    pub async fn deciphered_audio(&self, video_id: &str) -> Result<AudioFormat> {
        let solver = self.solver().await?;
        let client = match self.is_authenticated() {
            true => Client::TvDowngraded,
            false => Client::Tv,
        };
        log::debug!(
            "player: asking {} {} for {video_id}, player {} sts {}",
            client.name(),
            client.version(),
            solver.id(),
            solver.sts()
        );
        let payload = json!({
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
            "cpn": random_string(16),
            "playbackContext": {
                "contentPlaybackContext": {
                    "html5Preference": "HTML5_PREF_WANTS",
                    "signatureTimestamp": solver.sts(),
                }
            },
        });
        let response = self.visiting_player(client, payload).await?;
        let playability = playability(&response);
        if !playability.ok() {
            bail!(
                "{} is not playable: {} ({})",
                video_id,
                playability.status,
                playability.reason.as_deref().unwrap_or("no reason")
            );
        }
        let formats = response
            .get("streamingData")
            .and_then(|data| data.get("adaptiveFormats"))
            .and_then(Value::as_array)
            .context("player response has no adaptive formats")?;
        let mut audio: Vec<Ciphered> = formats.iter().filter_map(ciphered).collect();
        if audio.is_empty() {
            let sabr = response
                .pointer("/streamingData/serverAbrStreamingUrl")
                .is_some();
            match sabr && !self.is_authenticated() {
                true => bail!("{video_id} is served over sabr only; sign in to stream it"),
                false => bail!("{video_id} offers no addressable audio stream"),
            }
        }
        audio.sort_by_key(|format| std::cmp::Reverse(format.bitrate));
        let chosen = prefer_aac(audio)
            .with_context(|| format!("no ciphered audio stream for {video_id}"))?;
        let url = decipher(&solver, &chosen).await?;
        Ok(AudioFormat {
            itag: chosen.itag,
            url,
            mime: chosen.mime,
            codec: chosen.codec,
            bitrate: chosen.bitrate,
            duration: chosen.duration,
            content_length: chosen.content_length,
            loudness_db: chosen.loudness_db,
            user_agent: client.user_agent(),
        })
    }

    async fn visiting_player(&self, client: Client, payload: Value) -> Result<Value> {
        let primed = self.stream_visitor().await;
        let response = self
            .execute_visiting("player", client, payload.clone(), true, primed.as_deref())
            .await?;
        if playability(&response).status != "LOGIN_REQUIRED" || self.is_authenticated() {
            return Ok(response);
        }
        let Some(issued) = response
            .pointer("/responseContext/visitorData")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Ok(response);
        };
        log::debug!("player: primed a server-issued visitor for deciphering");
        self.set_stream_visitor(issued.clone()).await;
        self.execute_visiting("player", client, payload, true, Some(&issued))
            .await
    }

    async fn stream_player(&self, video_id: &str) -> Result<Value> {
        let primed = self.stream_visitor().await;
        let response = self.stream_request(video_id, primed.as_deref()).await?;
        if playability(&response).status != "LOGIN_REQUIRED" {
            return Ok(response);
        }
        let Some(issued) = response
            .pointer("/responseContext/visitorData")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Ok(response);
        };
        log::debug!("player: primed a server-issued visitor for streaming");
        self.set_stream_visitor(issued.clone()).await;
        self.stream_request(video_id, Some(&issued)).await
    }

    async fn stream_request(&self, video_id: &str, visitor: Option<&str>) -> Result<Value> {
        let mut client = json!({
            "clientName": "ANDROID_VR",
            "clientVersion": "1.65.10",
            "deviceMake": "Oculus",
            "deviceModel": "Quest 3",
            "androidSdkVersion": 32,
            "userAgent": VR_UA,
            "osName": "Android",
            "osVersion": "12L",
            "hl": self.lang(),
            "gl": self.region(),
            "timeZone": "UTC",
            "utcOffsetMinutes": 0,
        });
        if let Some(visitor) = visitor {
            client["visitorData"] = json!(visitor);
        }
        let body = json!({
            "context": { "client": client },
            "videoId": video_id,
            "playbackContext": {
                "contentPlaybackContext": { "html5Preference": "HTML5_PREF_WANTS" }
            },
            "contentCheckOk": true,
            "racyCheckOk": true,
        });
        let mut request = self
            .client()
            .post(PLAYER_URL)
            .header("User-Agent", VR_UA)
            .header("Content-Type", "application/json")
            .header("Origin", "https://www.youtube.com")
            .header("X-Youtube-Client-Name", "28")
            .header("X-Youtube-Client-Version", "1.65.10")
            .json(&body);
        if let Some(visitor) = visitor {
            request = request.header("X-Goog-Visitor-Id", visitor);
        }
        let response = request.send().await.context("cannot reach player")?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .context("cannot read player response")?;
        log::debug!("player via ANDROID_VR: {status}, {} bytes", bytes.len());
        serde_json::from_slice(&bytes)
            .with_context(|| format!("player returned non-json response with status {status}"))
    }

    pub async fn download(&self, format: &AudioFormat) -> Result<Vec<u8>> {
        let Some(total) = format.content_length else {
            return self.download_serial(format).await;
        };
        let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(PARALLEL));
        let mut tasks = tokio::task::JoinSet::new();
        let mut offset = 0u64;
        let mut index = 0usize;
        while offset < total {
            let end = (offset + CHUNK - 1).min(total - 1);
            let http = self.http.clone();
            let url = format.url.clone();
            let agent = format.user_agent;
            let limit = limit.clone();
            let start = offset;
            tasks.spawn(async move {
                let _permit = limit.acquire().await;
                (index, range(&http, &url, agent, start, end).await)
            });
            offset = end + 1;
            index += 1;
        }
        let mut parts: Vec<(usize, Vec<u8>)> = Vec::with_capacity(index);
        while let Some(joined) = tasks.join_next().await {
            let (index, chunk) = joined.context("download task did not finish")?;
            parts.push((index, chunk?));
        }
        parts.sort_by_key(|(index, _)| *index);
        let mut data = Vec::with_capacity(total as usize);
        for (_, chunk) in parts {
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }

    async fn download_serial(&self, format: &AudioFormat) -> Result<Vec<u8>> {
        let mut data = Vec::with_capacity((CHUNK * 2) as usize);
        let mut offset = 0u64;
        while offset < MAX_STREAM_BYTES {
            let end = offset + CHUNK - 1;
            let chunk = range(&self.http, &format.url, format.user_agent, offset, end).await?;
            let short = (chunk.len() as u64) < CHUNK;
            offset += chunk.len() as u64;
            data.extend_from_slice(&chunk);
            if short {
                break;
            }
        }
        Ok(data)
    }
}

async fn range(
    http: &reqwest::Client,
    url: &str,
    agent: &'static str,
    start: u64,
    end: u64,
) -> Result<Vec<u8>> {
    let response = http
        .get(url)
        .header("Range", format!("bytes={start}-{end}"))
        .header("User-Agent", agent)
        .header("Accept-Encoding", "identity")
        .header("Origin", "https://www.youtube.com")
        .header("Referer", "https://www.youtube.com/")
        .send()
        .await
        .context("cannot reach stream host")?;
    let status = response.status();
    if !status.is_success() {
        bail!(
            "stream host refused the download with status {status} \
             (a proof-of-origin token is likely required)"
        );
    }
    let chunk = response.bytes().await.context("cannot read stream chunk")?;
    Ok(chunk.to_vec())
}

struct Ciphered {
    itag: u32,
    mime: String,
    codec: String,
    bitrate: u32,
    duration: Option<Duration>,
    content_length: Option<u64>,
    loudness_db: Option<f32>,
    url: String,
    signature: Option<(String, String)>,
}

fn ciphered(format: &Value) -> Option<Ciphered> {
    let mime = format.get("mimeType").and_then(Value::as_str)?;
    if !mime.starts_with("audio/") {
        return None;
    }
    let (url, signature) = match format.get("signatureCipher").and_then(Value::as_str) {
        Some(cipher) => {
            let held = reqwest::Url::parse(&format!("https://cipher/?{cipher}")).ok()?;
            let url = param(&held, "url")?;
            let sig = param(&held, "s")?;
            let into = param(&held, "sp").unwrap_or_else(|| "signature".to_string());
            (url, Some((into, sig)))
        }
        None => (format.get("url").and_then(Value::as_str)?.to_string(), None),
    };
    let codec = mime
        .split_once("codecs=\"")
        .map(|(_, tail)| tail.trim_end_matches('"'))
        .unwrap_or_default();
    Some(Ciphered {
        itag: format.get("itag").and_then(Value::as_u64)? as u32,
        mime: mime.to_string(),
        codec: codec.to_string(),
        bitrate: format.get("bitrate").and_then(Value::as_u64).unwrap_or(0) as u32,
        duration: format
            .get("approxDurationMs")
            .and_then(Value::as_str)
            .and_then(|ms| ms.parse::<u64>().ok())
            .map(Duration::from_millis),
        content_length: format
            .get("contentLength")
            .and_then(Value::as_str)
            .and_then(|length| length.parse().ok()),
        loudness_db: format
            .get("loudnessDb")
            .and_then(Value::as_f64)
            .map(|db| db as f32),
        url,
        signature,
    })
}

async fn decipher(solver: &crate::deobf::Solver, format: &Ciphered) -> Result<String> {
    let mut url = reqwest::Url::parse(&format.url).context("the stream url does not parse")?;
    let throttle = param(&url, "n");
    let signature = format.signature.as_ref().map(|(_, sig)| sig.as_str());
    let started = std::time::Instant::now();
    let solved = solver.solve(signature, throttle.as_deref()).await?;
    log::debug!(
        "deobf: itag {} solved in {:?}, signature {} chars, n {} -> {}",
        format.itag,
        started.elapsed(),
        solved.sig.as_deref().map_or(0, str::len),
        throttle.as_deref().unwrap_or("none"),
        solved.n.as_deref().unwrap_or("none")
    );
    let mut changes: Vec<(&str, &str)> = Vec::with_capacity(2);
    if let Some((into, _)) = &format.signature {
        let sig = solved
            .sig
            .as_deref()
            .context("the solver returned no signature")?;
        changes.push((into.as_str(), sig));
    }
    if throttle.is_some() {
        let n = solved
            .n
            .as_deref()
            .context("the solver returned no n parameter")?;
        changes.push(("n", n));
    }
    set_params(&mut url, &changes);
    Ok(url.to_string())
}

fn describe(format: &AudioFormat, bytes: usize) -> String {
    format!(
        "itag {} {} {} kbps, {:.1} MiB",
        format.itag,
        format.codec,
        format.bitrate / 1000,
        bytes as f64 / (1024.0 * 1024.0)
    )
}

fn param(url: &reqwest::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

fn set_params(url: &mut reqwest::Url, changes: &[(&str, &str)]) {
    let existing: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    let mut query = url.query_pairs_mut();
    query.clear();
    for (name, value) in &existing {
        match changes.iter().find(|(key, _)| key == name) {
            Some((_, replacement)) => query.append_pair(name, replacement),
            None => query.append_pair(name, value),
        };
    }
    for (key, value) in changes {
        if !existing.iter().any(|(name, _)| name == key) {
            query.append_pair(key, value);
        }
    }
    query.finish();
}

fn prefer_aac(formats: Vec<Ciphered>) -> Option<Ciphered> {
    let aac = formats
        .iter()
        .position(|format| format.mime.starts_with("audio/mp4"));
    let mut formats = formats;
    match aac {
        Some(at) => Some(formats.swap_remove(at)),
        None => formats.into_iter().next(),
    }
}

fn pick(formats: Vec<AudioFormat>) -> Option<AudioFormat> {
    let aac = formats
        .iter()
        .filter(|format| format.mime.starts_with("audio/mp4"))
        .max_by_key(|format| format.bitrate)
        .cloned();
    aac.or_else(|| formats.into_iter().max_by_key(|format| format.bitrate))
}

fn playability(response: &Value) -> Playability {
    let status = response
        .get("playabilityStatus")
        .cloned()
        .unwrap_or_default();
    Playability {
        status: status
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
            .to_string(),
        reason: status
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn audio_format(format: &Value, client: Client) -> Option<AudioFormat> {
    let mime = format.get("mimeType").and_then(Value::as_str)?;
    if !mime.starts_with("audio/") {
        return None;
    }
    let url = format.get("url").and_then(Value::as_str)?;
    let codec = mime
        .split_once("codecs=\"")
        .map(|(_, tail)| tail.trim_end_matches('"'))
        .unwrap_or_default();
    Some(AudioFormat {
        itag: format.get("itag").and_then(Value::as_u64)? as u32,
        url: url.to_string(),
        mime: mime.to_string(),
        codec: codec.to_string(),
        bitrate: format.get("bitrate").and_then(Value::as_u64).unwrap_or(0) as u32,
        duration: format
            .get("approxDurationMs")
            .and_then(Value::as_str)
            .and_then(|ms| ms.parse::<u64>().ok())
            .map(Duration::from_millis),
        content_length: format
            .get("contentLength")
            .and_then(Value::as_str)
            .and_then(|length| length.parse().ok()),
        loudness_db: format
            .get("loudnessDb")
            .and_then(Value::as_f64)
            .map(|db| db as f32),
        user_agent: client.user_agent(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_ciphered_formats() {
        let format = json!({
            "itag": 140,
            "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
            "signatureCipher": "s=abc&sp=sig&url=https%3A%2F%2Fx",
            "bitrate": 130000,
        });
        assert!(audio_format(&format, Client::Ios).is_none());
    }

    #[test]
    fn parses_direct_format() {
        let format = json!({
            "itag": 140,
            "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
            "url": "https://example.com/stream",
            "bitrate": 130000,
            "contentLength": "3200000",
            "approxDurationMs": "185000",
            "loudnessDb": -4.5,
        });
        let parsed = audio_format(&format, Client::Ios).unwrap();
        assert_eq!(parsed.itag, 140);
        assert_eq!(parsed.codec, "mp4a.40.2");
        assert_eq!(parsed.duration, Some(Duration::from_secs(185)));
        assert_eq!(parsed.content_length, Some(3_200_000));
    }

    #[test]
    fn prefers_aac() {
        let formats = vec![
            AudioFormat {
                itag: 251,
                url: "opus".into(),
                mime: "audio/webm; codecs=\"opus\"".into(),
                codec: "opus".into(),
                bitrate: 160_000,
                duration: None,
                content_length: None,
                loudness_db: None,
                user_agent: "",
            },
            AudioFormat {
                itag: 140,
                url: "aac".into(),
                mime: "audio/mp4; codecs=\"mp4a.40.2\"".into(),
                codec: "mp4a.40.2".into(),
                bitrate: 130_000,
                duration: None,
                content_length: None,
                loudness_db: None,
                user_agent: "",
            },
        ];
        assert_eq!(pick(formats).unwrap().itag, 140);
    }
}
