//! 구글 ID 토큰 검증.
//!
//! 앱이 구글 로그인으로 받은 ID 토큰(RS256 JWT)을 구글 공개키(JWKS)로 확인한다.
//! 확인하는 것: 알고리즘 RS256, `kid`가 구글 키에 있음, 서명, `iss`, `aud`(허용한 클라이언트 ID),
//! `exp`, 이메일과 `email_verified == true`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::Mutex;

pub const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const ISSUERS: [&str; 2] = ["accounts.google.com", "https://accounts.google.com"];
/// 응답에 캐시 시간이 없을 때
const DEFAULT_CACHE: Duration = Duration::from_secs(60 * 60);
/// 모르는 `kid`가 와도 이보다 자주 다시 받지 않는다 (위조 토큰으로 구글을 두드리지 않게)
const MIN_REFETCH: Duration = Duration::from_secs(30);

/// 검증을 통과한 구글 신원.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleIdentity {
    pub sub: String,
    pub email: String,
    pub name: String,
    pub picture_url: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GoogleError {
    #[error("구글 토큰이 올바르지 않습니다: {0}")]
    Invalid(&'static str),
    #[error("이메일이 확인되지 않은 구글 계정입니다")]
    EmailNotVerified,
    #[error("구글 공개키를 받지 못했습니다")]
    KeysUnavailable,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    picture: Option<String>,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    n: Option<String>,
    e: Option<String>,
}

/// 구글 공개키를 어디서 얻는지.
pub enum KeySource {
    /// 고정된 키 (테스트)
    Static(HashMap<String, DecodingKey>),
    /// JWKS 주소에서 받아 캐시
    Remote(RemoteKeys),
}

pub struct RemoteKeys {
    http: reqwest::Client,
    url: String,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    keys: HashMap<String, DecodingKey>,
    expires: Option<Instant>,
    last_fetch: Option<Instant>,
}

impl RemoteKeys {
    pub fn new(url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("HTTP 클라이언트");
        RemoteKeys {
            http,
            url: url.into(),
            cache: Mutex::new(Cache::default()),
        }
    }

    async fn key(&self, kid: &str) -> Result<DecodingKey, GoogleError> {
        let mut cache = self.cache.lock().await;
        let now = Instant::now();
        let fresh = cache.expires.is_some_and(|e| now < e);
        if fresh {
            if let Some(k) = cache.keys.get(kid) {
                return Ok(k.clone());
            }
        }
        let may_fetch = cache.last_fetch.map_or(true, |t| now - t >= MIN_REFETCH);
        if may_fetch {
            cache.last_fetch = Some(now);
            match self.fetch().await {
                Ok((keys, ttl)) => {
                    cache.keys = keys;
                    cache.expires = Some(now + ttl);
                }
                Err(e) => {
                    tracing::warn!("google jwks fetch failed: {e}");
                    // 받아 둔 키가 있으면 만료됐어도 그대로 쓴다 (구글 장애 때 로그인 유지)
                    if cache.keys.is_empty() {
                        return Err(GoogleError::KeysUnavailable);
                    }
                }
            }
        }
        cache
            .keys
            .get(kid)
            .cloned()
            .ok_or(GoogleError::Invalid("unknown kid"))
    }

    async fn fetch(&self) -> anyhow::Result<(HashMap<String, DecodingKey>, Duration)> {
        let resp = self.http.get(&self.url).send().await?.error_for_status()?;
        let ttl = resp
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .and_then(max_age)
            .unwrap_or(DEFAULT_CACHE);
        let jwks: Jwks = resp.json().await?;
        let mut keys = HashMap::new();
        for k in jwks.keys {
            if k.kty != "RSA" {
                continue;
            }
            if let (Some(n), Some(e)) = (k.n, k.e) {
                if let Ok(dk) = DecodingKey::from_rsa_components(&n, &e) {
                    keys.insert(k.kid, dk);
                }
            }
        }
        anyhow::ensure!(!keys.is_empty(), "JWKS에 RSA 키가 없습니다");
        Ok((keys, ttl))
    }
}

/// `Cache-Control: public, max-age=19970, ...` 에서 max-age.
fn max_age(header: &str) -> Option<Duration> {
    header.split(',').find_map(|part| {
        let v = part.trim().strip_prefix("max-age=")?;
        v.parse::<u64>().ok().map(Duration::from_secs)
    })
}

pub struct GoogleVerifier {
    client_ids: Vec<String>,
    keys: KeySource,
}

impl GoogleVerifier {
    /// `client_ids`: 이 서버가 받아 줄 OAuth 클라이언트 ID (웹·데스크톱·Android·iOS).
    pub fn new(client_ids: Vec<String>, keys: KeySource) -> Self {
        GoogleVerifier { client_ids, keys }
    }

    pub fn client_ids(&self) -> &[String] {
        &self.client_ids
    }

    pub async fn verify(&self, id_token: &str) -> Result<GoogleIdentity, GoogleError> {
        if self.client_ids.is_empty() {
            return Err(GoogleError::Invalid("no client ids configured"));
        }
        let header =
            jsonwebtoken::decode_header(id_token).map_err(|_| GoogleError::Invalid("header"))?;
        if header.alg != Algorithm::RS256 {
            return Err(GoogleError::Invalid("alg"));
        }
        let kid = header.kid.ok_or(GoogleError::Invalid("kid"))?;
        let key = match &self.keys {
            KeySource::Static(m) => m
                .get(&kid)
                .cloned()
                .ok_or(GoogleError::Invalid("unknown kid"))?,
            KeySource::Remote(r) => r.key(&kid).await?,
        };
        let mut v = Validation::new(Algorithm::RS256);
        v.set_issuer(&ISSUERS);
        v.set_audience(&self.client_ids);
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        v.leeway = 60;
        let claims = jsonwebtoken::decode::<Claims>(id_token, &key, &v)
            .map_err(|_| GoogleError::Invalid("signature or claims"))?
            .claims;
        let email = claims.email.filter(|e| !e.is_empty());
        let Some(email) = email else {
            return Err(GoogleError::Invalid("email"));
        };
        if claims.email_verified != Some(true) {
            return Err(GoogleError::EmailNotVerified);
        }
        if claims.sub.is_empty() || claims.sub.len() > 255 {
            return Err(GoogleError::Invalid("sub"));
        }
        Ok(GoogleIdentity {
            sub: claims.sub,
            email,
            name: claims.name.unwrap_or_default(),
            picture_url: claims.picture.unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------- 데스크톱 인가 코드 교환 (F34)

pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// 데스크톱 앱(구글 "데스크톱 앱" 클라이언트)이 받은 인가 코드를 ID 토큰으로 바꾼다.
///
/// 구글은 데스크톱 클라이언트에도 토큰 교환 때 클라이언트 보안 비밀을 요구한다. 그 값을 앱에 넣지 않고
/// 이 서버만 갖는다(D-020). 앱은 PKCE(`code_verifier`)와 루프백 돌아올 주소로 코드를 받아 넘긴다.
pub struct CodeExchange {
    http: reqwest::Client,
    token_url: String,
    client_id: String,
    client_secret: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodeError {
    /// 코드가 틀렸거나 이미 썼거나 만료됨, `code_verifier`·돌아올 주소가 맞지 않음
    #[error("구글이 인가 코드를 받지 않았습니다")]
    Rejected,
    #[error("구글 토큰 주소에 닿지 못했습니다")]
    Unavailable,
}

#[derive(Deserialize)]
struct CodeResponse {
    id_token: Option<String>,
}

impl CodeExchange {
    pub fn new(
        token_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("HTTP 클라이언트");
        CodeExchange {
            http,
            token_url: token_url.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// 인가 코드 → 구글 ID 토큰(검증 전). 검증은 `GoogleVerifier::verify`가 한다.
    pub async fn exchange(
        &self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<String, CodeError> {
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", code_verifier),
            ("redirect_uri", redirect_uri),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
        ];
        let resp = self
            .http
            .post(&self.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|_| CodeError::Unavailable)?;
        let status = resp.status();
        if status.is_client_error() {
            return Err(CodeError::Rejected);
        }
        if !status.is_success() {
            return Err(CodeError::Unavailable);
        }
        let body: CodeResponse = resp.json().await.map_err(|_| CodeError::Unavailable)?;
        body.id_token
            .filter(|t| !t.is_empty())
            .ok_or(CodeError::Rejected)
    }
}

/// 데스크톱 앱의 돌아올 주소로 받아 주는 꼴: `http://127.0.0.1:<포트>` 또는 그 아래 경로.
/// 다른 주소로 코드를 바꿔 주지 않는다(남의 웹 페이지가 코드를 가로채 쓰지 못하게).
pub fn is_loopback_redirect(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("http://127.0.0.1:") else {
        return false;
    };
    let (port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let port_ok = !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p));
    let path_ok = path.len() <= 64
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_".contains(&b));
    port_ok && path_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_redirects_only() {
        for ok in [
            "http://127.0.0.1:53124",
            "http://127.0.0.1:53124/",
            "http://127.0.0.1:1/callback",
        ] {
            assert!(is_loopback_redirect(ok), "{ok}");
        }
        for bad in [
            "https://127.0.0.1:53124/",
            "http://localhost:53124/",
            "http://127.0.0.1/",
            "http://127.0.0.1:/",
            "http://127.0.0.1:0/",
            "http://127.0.0.1:70000/",
            "http://127.0.0.1:80@evil.com/",
            "http://127.0.0.1:80/?x=1",
            "http://127.0.0.1.evil.com:80/",
            "http://evil.com/",
        ] {
            assert!(!is_loopback_redirect(bad), "{bad}");
        }
    }

    #[test]
    fn parses_max_age() {
        assert_eq!(
            max_age("public, max-age=19970, must-revalidate, no-transform"),
            Some(Duration::from_secs(19970))
        );
        assert_eq!(max_age("no-cache"), None);
    }
}
