use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::client::YtMusic;
use crate::context::{Client, random_string};
use crate::models::AudioFormat;

const CHUNK: u64 = 1024 * 1024;
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
        serde_json::from_slice(&bytes)
            .with_context(|| format!("player returned non-json response with status {status}"))
    }

    pub async fn download(&self, format: &AudioFormat) -> Result<Vec<u8>> {
        let total = format.content_length.unwrap_or(MAX_STREAM_BYTES);
        let mut data = Vec::with_capacity(total.min(CHUNK * 2) as usize);
        let mut offset = 0u64;
        while offset < total {
            let end = (offset + CHUNK - 1).min(total - 1);
            let response = self
                .http
                .get(&format.url)
                .header("Range", format!("bytes={offset}-{end}"))
                .header("User-Agent", format.user_agent)
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
            if chunk.is_empty() {
                break;
            }
            let done = format.content_length.is_none() && (chunk.len() as u64) < CHUNK;
            offset += chunk.len() as u64;
            data.extend_from_slice(&chunk);
            if done {
                break;
            }
        }
        Ok(data)
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
