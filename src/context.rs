use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE;
use rand::Rng as _;
use serde_json::{Value, json};

pub const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

const ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Music,
    Tv,
    Ios,
    Android,
    AndroidVr,
}

impl Client {
    pub fn name(self) -> &'static str {
        match self {
            Self::Music => "WEB_REMIX",
            Self::Tv => "TVHTML5",
            Self::Ios => "IOS",
            Self::Android => "ANDROID",
            Self::AndroidVr => "ANDROID_VR",
        }
    }

    pub fn version(self) -> &'static str {
        match self {
            Self::Music => "1.20250219.01.00",
            Self::Tv => "7.20260311.12.00",
            Self::Ios => "20.11.6",
            Self::Android => "21.03.36",
            Self::AndroidVr => "1.65.10",
        }
    }

    pub fn id(self) -> u32 {
        match self {
            Self::Music => 67,
            Self::Tv => 7,
            Self::Ios => 5,
            Self::Android => 3,
            Self::AndroidVr => 28,
        }
    }

    pub fn user_agent(self) -> &'static str {
        match self {
            Self::Music => DESKTOP_UA,
            Self::Tv => "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
            Self::Ios => {
                "com.google.ios.youtube/20.11.6 (iPhone10,4; U; CPU iOS 16_7_7 like Mac OS X)"
            }
            Self::Android => {
                "com.google.android.youtube/21.03.36(Linux; U; Android 16; en_US; SM-S908E Build/TP1A.220624.014) gzip"
            }
            Self::AndroidVr => {
                "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip"
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
            Self::Ios => json!({
                "deviceMake": "Apple",
                "deviceModel": "iPhone10,4",
                "osName": "iOS",
                "osVersion": "16.7.7.20H330",
                "platform": "MOBILE",
            }),
            Self::Android => json!({
                "androidSdkVersion": 36,
                "osName": "Android",
                "osVersion": "16",
                "platform": "MOBILE",
            }),
            Self::AndroidVr => json!({
                "androidSdkVersion": 32,
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "osName": "Android",
                "osVersion": "12L",
                "platform": "MOBILE",
            }),
        };
        merge(&mut client, extra);
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

pub fn visitor_data(id: &str, timestamp: i64) -> String {
    let mut buffer = Vec::with_capacity(id.len() + 8);
    buffer.push(0x0A);
    buffer.push(id.len() as u8);
    buffer.extend_from_slice(id.as_bytes());
    buffer.push(0x28);
    write_varint(&mut buffer, timestamp as u64);
    URL_SAFE.encode(&buffer).replace('=', "%3D")
}

pub fn generate_visitor_data() -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    visitor_data(&random_string(11), timestamp)
}

fn write_varint(buffer: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        match value == 0 {
            true => {
                buffer.push(byte);
                break;
            }
            false => buffer.push(byte | 0x80),
        }
    }
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
    fn visitor_shape() {
        let data = visitor_data("CgtcTvBwOUNvcbg", 1_700_000_000);
        assert!(data.ends_with("%3D"));
        assert!(!data.contains('+'));
        assert!(!data.contains('/'));
    }

    #[test]
    fn context_music() {
        let ctx = Client::Music.context("abc", "en", "US");
        assert_eq!(ctx["client"]["clientName"], "WEB_REMIX");
        assert_eq!(ctx["client"]["visitorData"], "abc");
    }
}
