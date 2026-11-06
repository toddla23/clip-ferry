//! 접속 토큰 확인과 계정별 제한·사용량 (F33, D-019).
//!
//! - 계정 서버의 `GET /v1/keys`(Ed25519 공개키)로 접속 토큰 서명만 확인한다. DB는 보지 않는다.
//! - 계정별 동시 연결 수와 분당 바이트를 메모리에서 센다.
//! - 계정별 게시 수·바이트를 모아 주기적으로 계정 서버 `POST /internal/usage`로 보낸다.
//!   채널 ID·태그·페이로드는 보내지 않는다. 디스크에는 아무것도 쓰지 않는다.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::limits::Bucket;

/// 접속 토큰의 발급자·대상 (account/src/tokens.rs와 같아야 한다)
pub const ISSUER: &str = "clip-ferry-account";
pub const AUDIENCE: &str = "clip-ferry-relay";

/// 로그에 나오지 않는 비밀 문자열.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// 계정 서버 주소 (예: `http://127.0.0.1:8788`)
    pub account_url: String,
    /// 사용량 보고용 공유 비밀 (계정 서버의 `CF_ACCOUNT_USAGE_SECRET`). 없으면 보고하지 않는다.
    pub usage_secret: Option<Secret>,
    pub max_conns_per_account: usize,
    pub max_bytes_per_min_per_account: u64,
    /// 사용량을 보내는 간격
    pub usage_interval: Duration,
    /// 공개키를 다시 받는 간격
    pub keys_refresh: Duration,
}

impl AuthConfig {
    pub fn new(account_url: impl Into<String>) -> Self {
        AuthConfig {
            account_url: account_url.into().trim_end_matches('/').to_string(),
            usage_secret: None,
            max_conns_per_account: 16,
            max_bytes_per_min_per_account: 256 * 1024 * 1024,
            usage_interval: Duration::from_secs(60),
            keys_refresh: Duration::from_secs(10 * 60),
        }
    }
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    crv: Option<String>,
    x: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct UsageEntry {
    pub user_id: String,
    pub messages: u64,
    pub bytes: u64,
}

#[derive(Serialize)]
struct UsageReport<'a> {
    entries: &'a [UsageEntry],
}

/// 토큰 확인 결과
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 계정 ID
    Ok(String),
    Invalid,
    /// 아직 공개키를 받지 못했다 (계정 서버가 안 떠 있음)
    NotReady,
}

struct Account {
    conns: usize,
    bytes: Bucket,
    messages: u64,
    published_bytes: u64,
}

pub struct Auth {
    pub config: AuthConfig,
    keys: RwLock<HashMap<String, DecodingKey>>,
    accounts: Mutex<HashMap<String, Account>>,
    http: reqwest::Client,
}

impl Auth {
    pub fn new(config: AuthConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("HTTP 클라이언트");
        Auth {
            config,
            keys: RwLock::new(HashMap::new()),
            accounts: Mutex::new(HashMap::new()),
            http,
        }
    }

    pub fn ready(&self) -> bool {
        !self.keys.read().unwrap().is_empty()
    }

    /// `Authorization` 헤더 값(`Bearer …`)을 확인한다.
    pub fn verify_header(&self, header: Option<&str>) -> Verdict {
        if !self.ready() {
            return Verdict::NotReady;
        }
        match header.and_then(|h| h.strip_prefix("Bearer ")) {
            Some(token) => self.verify(token.trim()),
            None => Verdict::Invalid,
        }
    }

    fn verify(&self, token: &str) -> Verdict {
        let Ok(header) = jsonwebtoken::decode_header(token) else {
            return Verdict::Invalid;
        };
        if header.alg != Algorithm::EdDSA {
            return Verdict::Invalid;
        }
        let keys = self.keys.read().unwrap();
        let key = match &header.kid {
            Some(kid) => keys.get(kid),
            None if keys.len() == 1 => keys.values().next(),
            None => None,
        };
        let Some(key) = key else {
            return Verdict::Invalid;
        };
        let mut v = Validation::new(Algorithm::EdDSA);
        v.set_issuer(&[ISSUER]);
        v.set_audience(&[AUDIENCE]);
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        v.leeway = 30;
        match jsonwebtoken::decode::<Claims>(token, key, &v) {
            Ok(d) if !d.claims.sub.is_empty() && d.claims.sub.len() <= 64 => {
                Verdict::Ok(d.claims.sub)
            }
            _ => Verdict::Invalid,
        }
    }

    /// 계정 서버에서 공개키를 받아 바꾼다. 받은 키 수를 돌려준다.
    pub async fn refresh_keys(&self) -> anyhow::Result<usize> {
        let url = format!("{}/v1/keys", self.config.account_url);
        let jwks: Jwks = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut keys = HashMap::new();
        for k in jwks.keys {
            if k.kty != "OKP" || k.crv.as_deref() != Some("Ed25519") {
                continue;
            }
            let (Some(kid), Some(x)) = (k.kid, k.x) else {
                continue;
            };
            if let Ok(dk) = DecodingKey::from_ed_components(&x) {
                keys.insert(kid, dk);
            }
        }
        anyhow::ensure!(!keys.is_empty(), "계정 서버 공개키가 없습니다");
        let n = keys.len();
        *self.keys.write().unwrap() = keys;
        Ok(n)
    }

    /// 계정의 연결 자리를 얻는다. 한도를 넘으면 `None`.
    pub fn acquire(self: &Arc<Self>, user: String) -> Option<AccountGuard> {
        let mut accounts = self.accounts.lock().unwrap();
        let per_min = self.config.max_bytes_per_min_per_account as f64;
        let a = accounts.entry(user.clone()).or_insert_with(|| Account {
            conns: 0,
            bytes: Bucket::new(per_min, per_min / 60.0),
            messages: 0,
            published_bytes: 0,
        });
        if a.conns >= self.config.max_conns_per_account {
            return None;
        }
        a.conns += 1;
        Some(AccountGuard {
            auth: self.clone(),
            user,
        })
    }

    /// 계정의 분당 바이트 한도 안이면 쓰고 `true`.
    pub fn take_bytes(&self, user: &str, n: usize) -> bool {
        let mut accounts = self.accounts.lock().unwrap();
        accounts
            .get_mut(user)
            .map_or(true, |a| a.bytes.take(n as f64))
    }

    /// 게시 한 건을 사용량에 더한다.
    pub fn record_publish(&self, user: &str, payload_bytes: usize) {
        if let Some(a) = self.accounts.lock().unwrap().get_mut(user) {
            a.messages += 1;
            a.published_bytes += payload_bytes as u64;
        }
    }

    /// 모인 사용량을 꺼내고 0으로 돌린다. 연결이 없는 계정은 지운다.
    fn drain_usage(&self) -> Vec<UsageEntry> {
        let mut accounts = self.accounts.lock().unwrap();
        let mut out = Vec::new();
        for (user, a) in accounts.iter_mut() {
            if a.messages > 0 || a.published_bytes > 0 {
                out.push(UsageEntry {
                    user_id: user.clone(),
                    messages: std::mem::take(&mut a.messages),
                    bytes: std::mem::take(&mut a.published_bytes),
                });
            }
        }
        accounts.retain(|_, a| a.conns > 0);
        out
    }

    /// 보내지 못한 사용량을 되돌려 놓는다 (다음 번에 같이 보낸다).
    fn restore_usage(&self, entries: Vec<UsageEntry>) {
        let mut accounts = self.accounts.lock().unwrap();
        let per_min = self.config.max_bytes_per_min_per_account as f64;
        for e in entries {
            let a = accounts.entry(e.user_id).or_insert_with(|| Account {
                conns: 0,
                bytes: Bucket::new(per_min, per_min / 60.0),
                messages: 0,
                published_bytes: 0,
            });
            a.messages += e.messages;
            a.published_bytes += e.bytes;
        }
    }

    /// 모인 사용량을 계정 서버로 보낸다. 보낸 계정 수를 돌려준다.
    pub async fn report_usage(&self) -> anyhow::Result<usize> {
        let Some(secret) = &self.config.usage_secret else {
            return Ok(0);
        };
        let entries = self.drain_usage();
        if entries.is_empty() {
            return Ok(0);
        }
        let url = format!("{}/internal/usage", self.config.account_url);
        let result = async {
            self.http
                .post(url)
                .bearer_auth(&secret.0)
                .json(&UsageReport { entries: &entries })
                .send()
                .await?
                .error_for_status()?;
            anyhow::Ok(())
        }
        .await;
        match result {
            Ok(()) => Ok(entries.len()),
            Err(e) => {
                self.restore_usage(entries);
                Err(e)
            }
        }
    }
}

/// 계정의 동시 연결 자리. 드롭되면 돌려준다.
pub struct AccountGuard {
    auth: Arc<Auth>,
    pub user: String,
}

impl AccountGuard {
    pub fn auth(&self) -> &Auth {
        &self.auth
    }
}

impl Drop for AccountGuard {
    fn drop(&mut self) {
        let mut accounts = self.auth.accounts.lock().unwrap();
        if let Some(a) = accounts.get_mut(&self.user) {
            a.conns = a.conns.saturating_sub(1);
            if a.conns == 0 && a.messages == 0 && a.published_bytes == 0 {
                accounts.remove(&self.user);
            }
        }
    }
}

/// 공개키 갱신과 사용량 보고를 백그라운드에서 돌린다. 멈출 때 남은 사용량을 한 번 더 보낸다.
pub fn spawn_tasks(auth: Arc<Auth>, shutdown: watch::Receiver<bool>) {
    let keys_auth = auth.clone();
    let mut stop = shutdown.clone();
    tokio::spawn(async move {
        loop {
            let wait = match keys_auth.refresh_keys().await {
                Ok(n) => {
                    tracing::debug!("account keys loaded: {n}");
                    keys_auth.config.keys_refresh
                }
                Err(e) => {
                    tracing::warn!("account keys fetch failed: {e}");
                    // 받아 둔 키가 없으면 자주, 있으면 평소대로 다시 시도한다
                    if keys_auth.ready() {
                        keys_auth.config.keys_refresh
                    } else {
                        Duration::from_secs(2)
                    }
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stop.wait_for(|s| *s) => return,
            }
        }
    });

    if auth.config.usage_secret.is_none() {
        return;
    }
    let mut stop = shutdown;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(auth.config.usage_interval);
        tick.tick().await;
        loop {
            let stopping = tokio::select! {
                _ = tick.tick() => false,
                _ = stop.wait_for(|s| *s) => true,
            };
            if let Err(e) = auth.report_usage().await {
                tracing::warn!("usage report failed: {e}");
            }
            if stopping {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted() {
        let c = AuthConfig {
            usage_secret: Some(Secret("hunter2".into())),
            ..AuthConfig::new("http://x/")
        };
        let s = format!("{c:?}");
        assert!(!s.contains("hunter2"));
        assert_eq!(c.account_url, "http://x");
    }

    #[test]
    fn not_ready_without_keys() {
        let a = Auth::new(AuthConfig::new("http://127.0.0.1:1"));
        assert_eq!(a.verify_header(Some("Bearer x")), Verdict::NotReady);
    }

    #[tokio::test]
    async fn account_slots_and_usage() {
        let cfg = AuthConfig {
            max_conns_per_account: 1,
            ..AuthConfig::new("http://127.0.0.1:1")
        };
        let a = Arc::new(Auth::new(cfg));
        let g = a.acquire("u1".into()).unwrap();
        assert!(a.acquire("u1".into()).is_none());
        assert!(a.acquire("u2".into()).is_some());
        a.record_publish("u1", 10);
        a.record_publish("u1", 5);
        drop(g);
        // 연결이 끊겨도 보내기 전의 사용량은 남는다
        let mut usage = a.drain_usage();
        usage.sort_by(|x, y| x.user_id.cmp(&y.user_id));
        assert_eq!(
            usage,
            vec![UsageEntry {
                user_id: "u1".into(),
                messages: 2,
                bytes: 15
            }]
        );
        assert!(a.drain_usage().is_empty());
        assert!(a.acquire("u1".into()).is_some());
    }
}
