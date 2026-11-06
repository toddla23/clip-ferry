//! 앱 설정 (릴레이·계정 서버 주소, 동기화 켜고 끄기). 비밀이 아니므로 앱 데이터 폴더의 JSON 파일에 둔다.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_RELAY: &str = "ws://127.0.0.1:8787";
/// 계정 서버 기본 주소 (개발용. 공개 서버 주소는 F40에서 정한다)
pub const DEFAULT_ACCOUNT: &str = "http://127.0.0.1:8788";

/// 개발용 프로필 이름(`CF_DESKTOP_PROFILE`). 영문·숫자·`-`·`_`만 허용하고, 아니면 무시한다.
pub fn dev_profile() -> Option<String> {
    let name = std::env::var("CF_DESKTOP_PROFILE").ok()?;
    valid_profile(&name).then_some(name)
}

fn valid_profile(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub relay_url: String,
    /// 계정 서버 주소 (F34). 없던 파일은 `CF_ACCOUNT` 환경 변수나 기본값.
    #[serde(default = "default_account_url")]
    pub account_url: String,
    /// 비밀번호 관리자 등이 "보내지 말라"고 표시한 복사도 동기화한다 (기본: 예).
    /// 끄면 표시가 붙은 복사는 읽지도 보내지도 않는다. 끄는 화면은 만들지 않았고 `settings.json`에서 바꾼다.
    #[serde(default = "yes")]
    pub sync_sensitive: bool,
    /// 복사한 내용을 다른 기기로 보낸다 (기본: 예).
    #[serde(default = "yes")]
    pub send_enabled: bool,
    /// 다른 기기에서 온 내용을 클립보드에 쓴다 (기본: 예).
    #[serde(default = "yes")]
    pub receive_enabled: bool,
}

fn yes() -> bool {
    true
}

fn default_account_url() -> String {
    std::env::var("CF_ACCOUNT")
        .ok()
        .and_then(|v| validate_account_url(&v).ok())
        .unwrap_or_else(|| DEFAULT_ACCOUNT.to_string())
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            relay_url: DEFAULT_RELAY.to_string(),
            account_url: DEFAULT_ACCOUNT.to_string(),
            sync_sensitive: true,
            send_enabled: true,
            receive_enabled: true,
        }
    }
}

/// 릴레이 주소로 쓸 수 있는 문자열인지 확인하고 앞뒤 공백을 지운다.
pub fn validate_relay_url(input: &str) -> Result<String, String> {
    let url = input.trim();
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))
        .ok_or("주소는 ws:// 또는 wss:// 로 시작해야 합니다")?;
    if rest.is_empty() || rest.starts_with('/') || rest.chars().any(char::is_whitespace) {
        return Err("릴레이 주소 형식이 올바르지 않습니다".to_string());
    }
    Ok(url.to_string())
}

/// 계정 서버 주소로 쓸 수 있는지 확인하고 앞뒤 공백과 끝의 `/`를 지운다.
pub fn validate_account_url(input: &str) -> Result<String, String> {
    let url = input.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or("계정 서버 주소는 http:// 또는 https:// 로 시작해야 합니다")?;
    if rest.is_empty()
        || rest.starts_with('/')
        || rest
            .chars()
            .any(|c| c.is_whitespace() || c == '?' || c == '#')
    {
        return Err("계정 서버 주소 형식이 올바르지 않습니다".to_string());
    }
    Ok(url.to_string())
}

fn path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// 파일이 없거나 깨졌으면 기본값. `CF_RELAY` 환경 변수가 있으면 기본 주소로 쓴다.
pub fn load(dir: &Path) -> Settings {
    let from_file = std::fs::read(path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
        .filter(|s| validate_relay_url(&s.relay_url).is_ok())
        .map(|mut s| {
            if validate_account_url(&s.account_url).is_err() {
                s.account_url = default_account_url();
            }
            s
        });
    from_file.unwrap_or_else(|| {
        let relay_url = std::env::var("CF_RELAY")
            .ok()
            .and_then(|v| validate_relay_url(&v).ok())
            .unwrap_or_else(|| DEFAULT_RELAY.to_string());
        Settings {
            relay_url,
            account_url: default_account_url(),
            ..Settings::default()
        }
    })
}

pub fn save(dir: &Path, settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    std::fs::write(path(dir), bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ws_and_wss_and_trims() {
        assert_eq!(
            validate_relay_url("  ws://127.0.0.1:8787 ").unwrap(),
            "ws://127.0.0.1:8787"
        );
        assert!(validate_relay_url("wss://relay.example.com/ws").is_ok());
    }

    #[test]
    fn rejects_bad_urls() {
        for bad in [
            "",
            "http://x",
            "127.0.0.1:8787",
            "ws://",
            "ws:///x",
            "ws://a b",
        ] {
            assert!(validate_relay_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn account_urls() {
        assert_eq!(
            validate_account_url(" https://pi.example.ts.net:8443/ ").unwrap(),
            "https://pi.example.ts.net:8443"
        );
        assert!(validate_account_url("http://127.0.0.1:8788").is_ok());
        for bad in [
            "",
            "ws://x",
            "pi:8443",
            "http://",
            "https:///x",
            "http://a b",
            "http://x?y",
        ] {
            assert!(validate_account_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn profile_names_cannot_escape_the_data_dir() {
        assert!(valid_profile("t1") && valid_profile("dev_2-a"));
        for bad in ["", "..", "a/b", "a\\b", "a b", "한글"] {
            assert!(!valid_profile(bad), "{bad:?}");
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            relay_url: "ws://10.0.0.5:9000".to_string(),
            account_url: "https://pi.example.ts.net:8443".to_string(),
            sync_sensitive: false,
            send_enabled: false,
            receive_enabled: true,
        };
        save(dir.path(), &s).unwrap();
        assert_eq!(load(dir.path()), s);
    }

    #[test]
    fn old_settings_file_without_the_new_field_keeps_syncing_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"relay_url":"ws://10.0.0.5:9000"}"#,
        )
        .unwrap();
        let s = load(dir.path());
        assert_eq!(s.relay_url, "ws://10.0.0.5:9000");
        assert!(s.sync_sensitive && s.send_enabled && s.receive_enabled);
        assert!(
            validate_account_url(&s.account_url).is_ok(),
            "계정 서버 주소는 기본값"
        );
    }

    #[test]
    fn broken_or_invalid_file_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), b"not json").unwrap();
        assert!(validate_relay_url(&load(dir.path()).relay_url).is_ok());
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"relay_url":"http://x"}"#,
        )
        .unwrap();
        assert!(load(dir.path()).relay_url.starts_with("ws"));
    }
}
