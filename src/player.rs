use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::client::YtMusic;
use crate::context::{Client, random_string};
use crate::models::AudioFormat;

const STREAM_CLIENTS: [Client; 3] = [Client::AndroidVr, Client::Ios, Client::Android];

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
        self.execute(
            "player",
            client,
            json!({
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
            }),
        )
        .await
    }

    pub async fn audio_formats(&self, video_id: &str, client: Client) -> Result<Vec<AudioFormat>> {
        let response = self.player_response(video_id, client).await?;
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
        let mut audio: Vec<AudioFormat> = formats.iter().filter_map(audio_format).collect();
        audio.sort_by(|a, b| b.bitrate.cmp(&a.bitrate));
        Ok(audio)
    }

    pub async fn best_audio(&self, video_id: &str) -> Result<AudioFormat> {
        let mut last_error = None;
        for client in STREAM_CLIENTS {
            match self.audio_formats(video_id, client).await {
                Ok(formats) => {
                    if let Some(format) = pick(formats) {
                        return Ok(format);
                    }
                    last_error = Some(anyhow::anyhow!(
                        "no direct audio format from {}",
                        client.name()
                    ));
                }
                Err(error) => {
                    log::debug!("player: {} failed for {video_id}: {error:#}", client.name());
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no stream clients configured")))
            .with_context(|| format!("cannot resolve audio stream for {video_id}"))
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

fn audio_format(format: &Value) -> Option<AudioFormat> {
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
        assert!(audio_format(&format).is_none());
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
        let parsed = audio_format(&format).unwrap();
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
            },
        ];
        assert_eq!(pick(formats).unwrap().itag, 140);
    }
}
