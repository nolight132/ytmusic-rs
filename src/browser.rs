use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

const WANTED: &[&str] = &[
    "SID",
    "__Secure-1PSID",
    "__Secure-3PSID",
    "__Secure-1PSIDTS",
    "__Secure-3PSIDTS",
    "__Secure-1PSIDCC",
    "__Secure-3PSIDCC",
    "HSID",
    "SSID",
    "APISID",
    "SAPISID",
    "__Secure-1PAPISID",
    "__Secure-3PAPISID",
    "LOGIN_INFO",
    "SIDCC",
    "PREF",
    "VISITOR_INFO1_LIVE",
    "VISITOR_PRIVACY_METADATA",
    "__Secure-YNID",
    "__Secure-ROLLOUT_TOKEN",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Firefox,
    Chromium,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browser {
    pub name: &'static str,
    pub family: Family,
    pub root: PathBuf,
}

pub fn detect() -> Vec<Browser> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let config = config_dir(&home);
    let firefox = [
        ("Firefox", ".mozilla/firefox"),
        ("Zen", ".config/zen"),
        ("LibreWolf", ".librewolf"),
        ("Floorp", ".floorp"),
    ];
    let chromium = [
        ("Chromium", "chromium"),
        ("Chrome", "google-chrome"),
        ("Brave", "BraveSoftware/Brave-Browser"),
        ("Vivaldi", "vivaldi"),
        ("Edge", "microsoft-edge"),
        ("Helium", "net.imput.helium"),
    ];
    let mut found = Vec::new();
    for (name, rel) in firefox {
        let base = home.join(rel);
        if firefox_profile(&base).is_some() {
            found.push(Browser {
                name,
                family: Family::Firefox,
                root: base,
            });
        }
    }
    for (name, rel) in chromium {
        let base = config.join(rel);
        if base.join("Default/Cookies").exists() || base.join("Cookies").exists() {
            found.push(Browser {
                name,
                family: Family::Chromium,
                root: base,
            });
        }
    }
    found
}

pub fn cookies(browser: &Browser) -> Result<String> {
    match browser.family {
        Family::Firefox => firefox_cookies(&browser.root),
        Family::Chromium => chromium_cookies(&browser.root),
    }
}

fn firefox_cookies(root: &Path) -> Result<String> {
    let profile = firefox_profile(root).context("no firefox profile with cookies")?;
    let db = profile.join("cookies.sqlite");
    let temp = copy_locked(&db)?;
    let result = read_firefox(&temp);
    let _ = std::fs::remove_file(&temp);
    assemble(result?)
}

fn read_firefox(db: &Path) -> Result<Vec<(String, String)>> {
    let connection =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("cannot open cookies.sqlite")?;
    let mut statement = connection
        .prepare("SELECT name, value FROM moz_cookies WHERE host LIKE '%youtube.com'")
        .context("cannot query moz_cookies")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("cannot read cookie rows")?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn firefox_profile(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("cookies.sqlite").exists())
        .max_by_key(|path| {
            std::fs::metadata(path.join("cookies.sqlite"))
                .and_then(|meta| meta.modified())
                .ok()
        })
}

fn chromium_cookies(root: &Path) -> Result<String> {
    let db = ["Default/Cookies", "Cookies"]
        .iter()
        .map(|rel| root.join(rel))
        .find(|path| path.exists())
        .context("no chromium cookie store")?;
    let key = chromium_key(root);
    let temp = copy_locked(&db)?;
    let result = read_chromium(&temp, &key);
    let _ = std::fs::remove_file(&temp);
    assemble(result?)
}

fn read_chromium(db: &Path, key: &[u8]) -> Result<Vec<(String, String)>> {
    let connection =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("cannot open chromium cookies")?;
    let mut statement = connection
        .prepare("SELECT name, encrypted_value FROM cookies WHERE host_key LIKE '%youtube.com'")
        .context("cannot query chromium cookies")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .context("cannot read chromium rows")?;
    let mut pairs = Vec::new();
    for row in rows.flatten() {
        if let Some(value) = decrypt_chromium(&row.1, key) {
            pairs.push((row.0, value));
        }
    }
    Ok(pairs)
}

fn chromium_key(_root: &Path) -> Vec<u8> {
    use pbkdf2::pbkdf2_hmac;
    use sha1::Sha1;
    let mut key = [0u8; 16];
    pbkdf2_hmac::<Sha1>(b"peanuts", b"saltysalt", 1, &mut key);
    key.to_vec()
}

fn decrypt_chromium(value: &[u8], key: &[u8]) -> Option<String> {
    if value.len() < 3 {
        return String::from_utf8(value.to_vec()).ok();
    }
    let version = &value[..3];
    if version != b"v10" && version != b"v11" {
        return String::from_utf8(value.to_vec()).ok();
    }
    use aes::Aes128;
    use cbc::Decryptor;
    use cbc::cipher::{BlockDecryptMut as _, KeyIvInit as _, block_padding::Pkcs7};
    let iv = [0x20u8; 16];
    let plain = Decryptor::<Aes128>::new(key.into(), &iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(&value[3..])
        .ok()?;
    String::from_utf8(plain).ok()
}

fn assemble(pairs: Vec<(String, String)>) -> Result<String> {
    let mut cookie = String::new();
    for name in WANTED {
        if let Some((_, value)) = pairs.iter().find(|(candidate, _)| candidate == name) {
            if !cookie.is_empty() {
                cookie.push_str("; ");
            }
            cookie.push_str(name);
            cookie.push('=');
            cookie.push_str(value);
        }
    }
    if !cookie.contains("SAPISID=") && !cookie.contains("__Secure-3PAPISID=") {
        bail!("no youtube login cookies found; sign in to the browser first");
    }
    Ok(cookie)
}

fn copy_locked(db: &Path) -> Result<PathBuf> {
    let stamp = db
        .metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let temp = std::env::temp_dir().join(format!("ytmusic-cookies-{stamp}.sqlite"));
    std::fs::copy(db, &temp).with_context(|| format!("cannot copy {}", db.display()))?;
    Ok(temp)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn config_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
}
