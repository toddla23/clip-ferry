//! 계정 로그인 (F35, D-019). 모바일 셸이 받은 구글 ID 토큰 → 계정 서버 → 릴레이 접속 토큰.
//!
//! 데스크톱 `apps/desktop/src-tauri/src/account.rs`와 같은 규칙을 따른다(D-020):
//! - 갱신 토큰은 플랫폼 보안 저장소(항목 `account/session`), 접속 토큰(1시간)은 메모리에만 둔다
//! - 갱신은 한 번에 하나씩만 한다. 같은 갱신 토큰을 두 번 보내면 계정 서버가 재사용으로 보고
//!   그 계정의 모든 로그인을 끊는다
//! - 계정 서버가 갱신 토큰을 거절하면(401) 로그인 정보를 지우고 "다시 로그인" 상태가 된다.
//!   닿지 않으면(네트워크) 로그인은 그대로 두고 나중에 다시 시도한다
//!
//! 구글 로그인 화면은 셸이 띄운다(Android: Credential Manager). 이 모듈은 ID 토큰만 받는다.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use clip_ferry_core::link::AccountLink;
use clip_ferry_core::pb;
use clip_ferry_core::store::SecretStore;
use serde::{Deserialize, Serialize};

/// 보안 저장소의 항목 이름 (값은 `Saved`의 JSON). 데스크톱과 같은 이름이다.
pub(crate) const KEY: &str = "account/session";
/// 접속 토큰 만료까지 이보다 적게 남았으면 갱신한다
const REFRESH_MARGIN_SECS: i64 = 120;
/// 릴레이가 토큰을 거절해도, 받은 지 이만큼 안 된 토큰은 버리지 않는다(갱신을 되풀이하지 않게)
const MIN_TOKEN_AGE_SECS: i64 = 30;

#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    account_url: String,
    refresh_token: String,
    email: String,
    #[serde(default)]
    name: String,
    /// 계정 ID (F37 계정 연결). F35 때 저장한 정보에는 없어서 처음 쓸 때 `/v1/me`로 채운다.
    #[serde(default)]
    user_id: String,
}

#[derive(Clone)]
struct Access {
    token: String,
    /// 만료 (Unix 초)
    expires_at: i64,
    /// 받은 시각 (Unix 초)
    issued_at: i64,
}

#[derive(Default)]
struct Data {
    saved: Option<Saved>,
    access: Option<Access>,
}

/// 화면에 보여 줄 로그인 정보
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Profile {
    pub email: String,
    pub name: String,
    pub account_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenError {
    /// 로그인하지 않았거나 계정 서버가 갱신 토큰을 받지 않았다(만료·로그아웃·재사용) → 다시 로그인
    SignedOut,
    /// 계정 서버에 닿지 못했다. 나중에 다시 시도한다
    Unavailable,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: String,
    user: UserView,
}

#[derive(Deserialize)]
struct UserView {
    #[serde(default)]
    id: String,
    email: String,
    #[serde(default)]
    name: String,
}

pub(crate) struct Account {
    http: reqwest::Client,
    secrets: Box<dyn SecretStore>,
    data: Mutex<Data>,
    /// 갱신·로그인·로그아웃을 한 번에 하나씩
    one: tokio::sync::Mutex<()>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 계정 서버 주소를 다듬는다. `http://` 또는 `https://`만 받는다.
pub(crate) fn check_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(url.to_string())
    } else {
        Err("계정 서버 주소는 http:// 또는 https://로 시작해야 합니다".into())
    }
}

impl Account {
    /// 보안 저장소에 로그인 정보가 있으면 읽어 둔다.
    pub fn new(secrets: Box<dyn SecretStore>) -> Self {
        let saved = secrets
            .get(KEY)
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok());
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("HTTP 클라이언트");
        Account {
            http,
            secrets,
            data: Mutex::new(Data {
                saved,
                access: None,
            }),
            one: tokio::sync::Mutex::new(()),
        }
    }

    pub fn profile(&self) -> Option<Profile> {
        self.data.lock().unwrap().saved.as_ref().map(|s| Profile {
            email: s.email.clone(),
            name: s.name.clone(),
            account_url: s.account_url.clone(),
        })
    }

    pub fn is_signed_in(&self) -> bool {
        self.data.lock().unwrap().saved.is_some()
    }

    /// 쓸 수 있는 접속 토큰. 만료가 가까우면 갱신한다.
    pub async fn token(&self) -> Result<String, TokenError> {
        let _one = self.one.lock().await;
        let (saved, access) = {
            let d = self.data.lock().unwrap();
            (d.saved.clone(), d.access.clone())
        };
        let saved = saved.ok_or(TokenError::SignedOut)?;
        if let Some(a) = access {
            if a.expires_at - now_secs() > REFRESH_MARGIN_SECS {
                return Ok(a.token);
            }
        }
        let resp = self
            .http
            .post(format!("{}/v1/auth/refresh", saved.account_url))
            .json(&serde_json::json!({ "refresh_token": saved.refresh_token }))
            .send()
            .await
            .map_err(|_| TokenError::Unavailable)?;
        match resp.status().as_u16() {
            200 => {}
            401 => {
                self.forget();
                return Err(TokenError::SignedOut);
            }
            _ => return Err(TokenError::Unavailable),
        }
        let r: TokenResponse = resp.json().await.map_err(|_| TokenError::Unavailable)?;
        Ok(self.keep(&saved.account_url, r))
    }

    /// 릴레이가 접속 토큰을 거절했다(서명 키가 바뀌었을 수 있다). 다음에는 새로 받는다.
    pub fn invalidate_access(&self) {
        let mut d = self.data.lock().unwrap();
        if d.access
            .as_ref()
            .is_some_and(|a| now_secs() - a.issued_at >= MIN_TOKEN_AGE_SECS)
        {
            d.access = None;
        }
    }

    /// 받은 토큰을 메모리와 보안 저장소에 두고 접속 토큰을 돌려준다.
    fn keep(&self, account_url: &str, r: TokenResponse) -> String {
        let now = now_secs();
        let saved = Saved {
            account_url: account_url.to_string(),
            refresh_token: r.refresh_token,
            email: r.user.email,
            name: r.user.name,
            user_id: r.user.id,
        };
        // 저장에 실패하면 다음 실행 때 옛 갱신 토큰을 쓰게 되어(재사용) 로그인이 끊긴다.
        // 그래도 지금은 메모리의 새 토큰으로 계속 동작한다.
        if let Ok(bytes) = serde_json::to_vec(&saved) {
            let _ = self.secrets.set(KEY, &bytes);
        }
        let token = r.access_token;
        let mut d = self.data.lock().unwrap();
        d.saved = Some(saved);
        d.access = Some(Access {
            token: token.clone(),
            expires_at: now + r.expires_in,
            issued_at: now,
        });
        token
    }

    fn account_url(&self) -> Result<String, TokenError> {
        self.data
            .lock()
            .unwrap()
            .saved
            .as_ref()
            .map(|s| s.account_url.clone())
            .ok_or(TokenError::SignedOut)
    }

    /// 계정 연결(F37)에 쓸 계정 ID와 계정 서버 공개키(`GET /v1/keys`). 데스크톱과 같다.
    pub async fn account_link(&self) -> Result<AccountLink, TokenError> {
        let url = self.account_url()?;
        let mut user_id = self
            .data
            .lock()
            .unwrap()
            .saved
            .as_ref()
            .map(|s| s.user_id.clone())
            .unwrap_or_default();
        if user_id.is_empty() {
            #[derive(Deserialize)]
            struct Me {
                user: UserView,
            }
            let token = self.token().await?;
            let me: Me = self
                .http
                .get(format!("{url}/v1/me"))
                .bearer_auth(token)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|_| TokenError::Unavailable)?
                .json()
                .await
                .map_err(|_| TokenError::Unavailable)?;
            user_id = me.user.id;
            let mut d = self.data.lock().unwrap();
            if let Some(s) = d.saved.as_mut() {
                s.user_id = user_id.clone();
                if let Ok(bytes) = serde_json::to_vec(s) {
                    let _ = self.secrets.set(KEY, &bytes);
                }
            }
        }
        if user_id.is_empty() {
            return Err(TokenError::Unavailable);
        }
        #[derive(Deserialize)]
        struct Jwk {
            kid: String,
            x: String,
        }
        #[derive(Deserialize)]
        struct Jwks {
            keys: Vec<Jwk>,
        }
        let jwks: Jwks = self
            .http
            .get(format!("{url}/v1/keys"))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|_| TokenError::Unavailable)?
            .json()
            .await
            .map_err(|_| TokenError::Unavailable)?;
        let server_keys = jwks
            .keys
            .into_iter()
            .filter_map(|k| {
                let x: [u8; 32] = URL_SAFE_NO_PAD.decode(k.x).ok()?.try_into().ok()?;
                Some((k.kid, x))
            })
            .collect();
        Ok(AccountLink {
            user_id,
            server_keys,
        })
    }

    /// 계정 서버에서 이 기기의 참여 확인서를 받는다 (`POST /v1/link/ticket`, F37).
    pub async fn link_ticket(
        &self,
        sign_pub: &[u8; 32],
        dh_pub: &[u8; 32],
        name: &str,
    ) -> Result<pb::LinkTicket, String> {
        let url = self
            .account_url()
            .map_err(|_| "먼저 로그인하세요".to_string())?;
        let token = self.token().await.map_err(|e| match e {
            TokenError::SignedOut => "로그인이 만료되었습니다. 다시 로그인하세요".to_string(),
            TokenError::Unavailable => format!("계정 서버({url})에 연결하지 못했습니다"),
        })?;
        let resp = self
            .http
            .post(format!("{url}/v1/link/ticket"))
            .bearer_auth(token)
            .json(&serde_json::json!({
                "sign_pub": URL_SAFE_NO_PAD.encode(sign_pub),
                "dh_pub": URL_SAFE_NO_PAD.encode(dh_pub),
                "name": name,
            }))
            .send()
            .await
            .map_err(|_| format!("계정 서버({url})에 연결하지 못했습니다"))?;
        match resp.status().as_u16() {
            200 => {}
            404 => return Err("계정 서버가 계정 연결을 받지 않습니다 (예전 버전)".into()),
            s => return Err(format!("계정 서버 오류 (HTTP {s})")),
        }
        #[derive(Deserialize)]
        struct Ticket {
            user_id: String,
            sign_pub: String,
            dh_pub: String,
            name: String,
            expires_at_ms: u64,
            kid: String,
            signature: String,
        }
        let t: Ticket = resp
            .json()
            .await
            .map_err(|_| "계정 서버 응답을 읽지 못했습니다".to_string())?;
        let bytes = |s: &str| {
            URL_SAFE_NO_PAD
                .decode(s)
                .map_err(|_| "계정 서버 응답을 읽지 못했습니다".to_string())
        };
        Ok(pb::LinkTicket {
            user_id: t.user_id,
            sign_pub: bytes(&t.sign_pub)?,
            dh_pub: bytes(&t.dh_pub)?,
            name: t.name,
            expires_at_ms: t.expires_at_ms,
            kid: t.kid,
            signature: bytes(&t.signature)?,
        })
    }

    /// 이 기기에서 로그인 정보를 지운다 (서버에는 알리지 않는다).
    fn forget(&self) {
        *self.data.lock().unwrap() = Data::default();
        let _ = self.secrets.delete(KEY);
    }

    /// 로그아웃: 계정 서버에 그 로그인의 갱신 토큰을 무효화하라고 알리고(실패해도) 로컬에서 지운다.
    pub async fn logout(&self) {
        let _one = self.one.lock().await;
        let saved = self.data.lock().unwrap().saved.clone();
        self.forget();
        if let Some(s) = saved {
            let _ = self
                .http
                .post(format!("{}/v1/auth/logout", s.account_url))
                .timeout(Duration::from_secs(5))
                .json(&serde_json::json!({ "refresh_token": s.refresh_token }))
                .send()
                .await;
        }
    }

    /// Android 앱이 구글 로그인 창에 넘길 웹 클라이언트 ID (`GET /v1/auth/google/android`).
    pub async fn google_client_id(&self, account_url: &str) -> Result<String, String> {
        let account_url = check_url(account_url)?;
        let resp = self
            .http
            .get(format!("{account_url}/v1/auth/google/android"))
            .send()
            .await
            .map_err(|_| format!("계정 서버({account_url})에 연결하지 못했습니다"))?;
        match resp.status().as_u16() {
            200 => {}
            404 => return Err("계정 서버가 Android 구글 로그인을 받지 않습니다 (웹 클라이언트 ID가 설정되지 않음)".into()),
            s => return Err(format!("계정 서버 오류 (HTTP {s})")),
        }
        #[derive(Deserialize)]
        struct Config {
            client_id: String,
        }
        resp.json::<Config>()
            .await
            .map(|c| c.client_id)
            .map_err(|_| "계정 서버 응답을 읽지 못했습니다".into())
    }

    /// 구글 ID 토큰으로 로그인한다 (`POST /v1/auth/google`).
    pub async fn google_login(&self, account_url: &str, id_token: &str) -> Result<Profile, String> {
        let account_url = check_url(account_url)?;
        let resp = self
            .http
            .post(format!("{account_url}/v1/auth/google"))
            .json(&serde_json::json!({ "id_token": id_token }))
            .send()
            .await
            .map_err(|_| format!("계정 서버({account_url})에 연결하지 못했습니다"))?;
        self.finish(&account_url, resp).await
    }

    /// 개발용 로그인 (계정 서버 `--dev-login`, 구글 없이 이메일만). 셸은 디버그 빌드에서만 보여 준다.
    pub async fn dev_login(&self, account_url: &str, email: &str) -> Result<Profile, String> {
        let account_url = check_url(account_url)?;
        let resp = self
            .http
            .post(format!("{account_url}/v1/auth/dev"))
            .json(&serde_json::json!({ "email": email.trim(), "name": "" }))
            .send()
            .await
            .map_err(|_| format!("계정 서버({account_url})에 연결하지 못했습니다"))?;
        if resp.status().as_u16() == 404 {
            return Err("계정 서버가 개발용 로그인을 받지 않습니다 (--dev-login)".into());
        }
        self.finish(&account_url, resp).await
    }

    async fn finish(&self, account_url: &str, resp: reqwest::Response) -> Result<Profile, String> {
        let status = resp.status().as_u16();
        if status != 200 {
            #[derive(Deserialize)]
            struct Err {
                error: String,
            }
            let code = resp
                .json::<Err>()
                .await
                .map(|e| e.error)
                .unwrap_or_default();
            return Err(match code.as_str() {
                "email_not_verified" => "이메일이 확인되지 않은 구글 계정입니다".into(),
                "invalid_id_token" => "구글 로그인을 확인하지 못했습니다. 다시 시도하세요".into(),
                "google_unavailable" => "계정 서버가 구글에 닿지 못했습니다".into(),
                _ => format!("로그인 실패 (HTTP {status} {code})"),
            });
        }
        let r: TokenResponse = resp
            .json()
            .await
            .map_err(|_| "계정 서버 응답을 읽지 못했습니다".to_string())?;
        let _one = self.one.lock().await;
        self.keep(account_url, r);
        self.profile()
            .ok_or_else(|| "로그인 정보를 저장하지 못했습니다".into())
    }

    /// 이 기기를 계정의 기기 목록에 올린다(이름·플랫폼). `device_id`는 32자 hex.
    pub async fn register_device(
        &self,
        device_id: &str,
        name: &str,
        platform: &str,
    ) -> Result<(), TokenError> {
        let token = self.token().await?;
        let url = self
            .data
            .lock()
            .unwrap()
            .saved
            .as_ref()
            .map(|s| s.account_url.clone())
            .ok_or(TokenError::SignedOut)?;
        let resp = self
            .http
            .put(format!("{url}/v1/devices/{device_id}"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "name": name, "platform": platform }))
            .send()
            .await
            .map_err(|_| TokenError::Unavailable)?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(TokenError::Unavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_ferry_core::store::MemorySecretStore;

    #[test]
    fn checks_account_url() {
        assert_eq!(
            check_url(" https://pi.ts.net:8443/ ").unwrap(),
            "https://pi.ts.net:8443"
        );
        assert!(check_url("ws://x").is_err());
        assert!(check_url("").is_err());
    }

    #[test]
    fn fresh_tokens_are_not_dropped_on_relay_rejection() {
        let acc = Account::new(Box::new(MemorySecretStore::default()));
        let now = now_secs();
        acc.data.lock().unwrap().access = Some(Access {
            token: "t".into(),
            expires_at: now + 3600,
            issued_at: now,
        });
        acc.invalidate_access();
        assert!(acc.data.lock().unwrap().access.is_some());
        acc.data.lock().unwrap().access.as_mut().unwrap().issued_at = now - 60;
        acc.invalidate_access();
        assert!(acc.data.lock().unwrap().access.is_none());
    }
}
