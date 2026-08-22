use rand::Rng as _;
use serde_json::{Value, json};

const ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Music,
    Tv,
    VisionOs,
}

impl Client {
    pub fn name(self) -> &'static str {
        match self {
            Self::Music => "WEB_REMIX",
            Self::Tv => "TVHTML5",
            Self::VisionOs => "VISIONOS",
        }
    }

    pub fn version(self) -> &'static str {
        match self {
            Self::Music => "1.20260707.12.00",
            Self::Tv => "7.20260707.07.00",
            Self::VisionOs => "1.02",
        }
    }

    pub fn id(self) -> u32 {
        match self {
            Self::Music => 67,
            Self::Tv => 7,
            Self::VisionOs => 101,
        }
    }

    pub fn user_agent(self) -> &'static str {
        match self {
            Self::Music => {
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36"
            }
            Self::Tv => "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
            Self::VisionOs => {
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15"
            }
        }
    }

    pub fn context(self, visitor_data: &str, hl: &str, gl: &str) -> Value {
        let mut client = json!({
            "hl": hl,
            "gl": gl,
            "visitorData": visitor_data,
            "clientName": self.name(),
            "clientVersion": self.version(),
            "userAgent": self.user_agent(),
            "utcOffsetMinutes": 0,
        });
        let extra = match self {
            Self::Music => json!({
                "osName": "Windows",
                "osVersion": "10.0",
                "platform": "DESKTOP",
                "clientFormFactor": "UNKNOWN_FORM_FACTOR",
                "browserName": "Chrome",
                "browserVersion": "125.0.0.0",
                "originalUrl": "https://music.youtube.com",
            }),
            Self::Tv => json!({
                "platform": "TV",
                "clientFormFactor": "UNKNOWN_FORM_FACTOR",
            }),
            Self::VisionOs => json!({
                "deviceMake": "Apple",
                "deviceModel": "RealityDevice17,1",
                "osName": "visionOS",
                "osVersion": "26.5.23O471",
            }),
        };
        merge(&mut client, extra);
        if visitor_data.is_empty()
            && let Some(map) = client.as_object_mut()
        {
            map.remove("visitorData");
        }
        json!({
            "client": client,
            "user": { "enableSafetyMode": false, "lockedSafetyMode": false },
            "request": { "useSsl": true, "internalExperimentFlags": [] },
        })
    }
}

pub fn random_string(length: usize) -> String {
    let mut rng = rand::rng();
    (0..length)
        .map(|_| ID_ALPHABET[rng.random_range(0..ID_ALPHABET.len())] as char)
        .collect()
}

fn merge(target: &mut Value, source: Value) {
    if let (Value::Object(target), Value::Object(source)) = (target, source) {
        for (key, value) in source {
            target.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_music() {
        let ctx = Client::Music.context("abc", "en", "US");
        assert_eq!(ctx["client"]["clientName"], "WEB_REMIX");
        assert_eq!(ctx["client"]["visitorData"], "abc");
    }

    #[test]
    fn context_drops_an_empty_visitor() {
        let ctx = Client::VisionOs.context("", "en", "US");
        assert_eq!(ctx["client"]["clientName"], "VISIONOS");
        assert!(ctx["client"].get("visitorData").is_none());
    }
}
