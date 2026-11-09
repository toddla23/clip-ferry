//! 계정 로그인 (F34, D-019). 구글 로그인 → 계정 서버 → 릴레이 접속 토큰.
//!
//! 흐름(구글 "데스크톱 앱" 클라이언트, RFC 8252):
//! 1. 계정 서버에서 데스크톱 클라이언트 ID를 받는다 (`GET /v1/auth/google/desktop`)
//! 2. `127.0.0.1:<임의 포트>`에서 기다리고, 시스템 브라우저로 구글 로그인 화면을 연다 (PKCE S256, `state`)
//! 3. 브라우저가 돌아오면 인가 코드를 계정 서버에 넘긴다 (`POST /v1/auth/google/code`).
//!    구글과의 코드 교환은 계정 서버가 한다(클라이언트 보안 비밀은 앱에 넣지 않는다, D-020)
//! 4. 받은 갱신 토큰은 OS 보안 저장소에, 접속 토큰(1시간)은 메모리에만 둔다
//!
//! 갱신은 한 번에 하나씩만 한다. 같은 갱신 토큰을 두 번 보내면 계정 서버가 재사용으로 보고
//! 그 계정의 모든 로그인을 끊는다.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use clip_ferry_core::link::AccountLink;
use clip_ferry_core::pb;
use clip_ferry_core::store::SecretStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 보안 저장소의 항목 이름 (값은 `Saved`의 JSON)
const KEY: &str = "account/session";
/// 접속 토큰 만료까지 이보다 적게 남았으면 갱신한다
const REFRESH_MARGIN_SECS: i64 = 120;
/// 릴레이가 토큰을 거절해도, 받은 지 이만큼 안 된 토큰은 버리지 않는다(갱신을 되풀이하지 않게)
const MIN_TOKEN_AGE_SECS: i64 = 30;
/// 브라우저에서 로그인을 마칠 때까지 기다리는 시간
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// 이 앱이 계정 서버에 알리는 플랫폼 이름
pub const PLATFORM: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "macos") {
    "macos"
} else {
    "linux"
};

/// 보안 저장소에 두는 로그인 정보. 접속 토큰은 두지 않는다.
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    account_url: String,
    refresh_token: String,
    email: String,
    #[serde(default)]
    name: String,
    /// 계정 ID (F37 계정 연결). F34 때 저장한 정보에는 없어서 처음 쓸 때 `/v1/me`로 채운다.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    pub email: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
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

pub struct Account {
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

fn base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
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

    /// 계정 연결(F37)에 쓸 계정 ID와 계정 서버 공개키(`GET /v1/keys`).
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
            // F34 때 로그인한 정보에는 계정 ID가 없다. 한 번 물어 저장해 둔다.
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

    /// 구글 로그인. `open`은 시스템 브라우저로 주소를 연다. 끝날 때까지(최대 5분) 기다린다.
    pub async fn google_login(
        &self,
        account_url: &str,
        open: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<Profile, String> {
        let account_url = base(account_url);
        let unreachable = |_| format!("계정 서버({account_url})에 연결하지 못했습니다");
        let resp = self
            .http
            .get(format!("{account_url}/v1/auth/google/desktop"))
            .send()
            .await
            .map_err(unreachable)?;
        match resp.status().as_u16() {
            200 => {}
            404 => return Err("계정 서버가 데스크톱 구글 로그인을 받지 않습니다 (데스크톱 클라이언트 ID가 설정되지 않음)".into()),
            s => return Err(format!("계정 서버 오류 (HTTP {s})")),
        }
        #[derive(Deserialize)]
        struct Config {
            client_id: String,
        }
        let client_id = resp
            .json::<Config>()
            .await
            .map_err(|_| "계정 서버 응답을 읽지 못했습니다".to_string())?
            .client_id;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "로그인 응답을 받을 포트를 열지 못했습니다".to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/");
        let pkce = Pkce::new();
        let state = random_b64(16);
        open(&auth_url(
            &client_id,
            &redirect_uri,
            &pkce.challenge,
            &state,
        ))?;

        let code = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_code(&listener, &state))
            .await
            .map_err(|_| "로그인 시간이 지났습니다. 다시 시도하세요".to_string())??;
        drop(listener);

        let resp = self
            .http
            .post(format!("{account_url}/v1/auth/google/code"))
            .json(&serde_json::json!({
                "code": code,
                "code_verifier": pkce.verifier,
                "redirect_uri": redirect_uri,
            }))
            .send()
            .await
            .map_err(unreachable)?;
        self.finish(&account_url, resp).await
    }

    /// 개발용 로그인 (계정 서버 `--dev-login`, 구글 없이 이메일만). 디버그 빌드에서만 쓴다.
    pub async fn dev_login(&self, account_url: &str, email: &str) -> Result<Profile, String> {
        let account_url = base(account_url);
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
                "invalid_code" | "invalid_id_token" => {
                    "구글 로그인을 확인하지 못했습니다. 다시 시도하세요".into()
                }
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
    pub async fn register_device(&self, device_id: &str, name: &str) -> Result<(), TokenError> {
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
            .json(&serde_json::json!({ "name": name, "platform": PLATFORM }))
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

// ---------------------------------------------------------------- OAuth 도우미

fn random_b64(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).expect("OS 난수를 얻지 못했습니다");
    URL_SAFE_NO_PAD.encode(b)
}

/// PKCE (RFC 7636, S256)
struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    fn new() -> Self {
        // 32바이트 → base64url 43자
        Self::from_verifier(random_b64(32))
    }

    fn from_verifier(verifier: String) -> Self {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

/// URL 쿼리 값 인코딩 (RFC 3986 unreserved만 그대로)
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn auth_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    format!(
        "{GOOGLE_AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}\
         &code_challenge={}&code_challenge_method=S256&state={}&prompt=select_account",
        enc(client_id),
        enc(redirect_uri),
        enc("openid email profile"),
        enc(challenge),
        enc(state),
    )
}

/// `GET /?a=1&b=2 HTTP/1.1` → 경로와 쿼리 값들
fn parse_request_line(line: &str) -> Option<(String, Vec<(String, String)>)> {
    let mut parts = line.split(' ');
    if parts.next()? != "GET" {
        return None;
    }
    let target = parts.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (dec(k), dec(v))
        })
        .collect();
    Some((path.to_string(), params))
}

/// 루프백으로 돌아온 브라우저 요청을 받아 인가 코드를 꺼낸다.
/// `state`가 맞지 않는 요청이나 다른 경로(`/favicon.ico`)는 무시하고 계속 기다린다.
async fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let Some(line) = read_request_line(&mut stream).await else {
            continue;
        };
        let Some((path, params)) = parse_request_line(&line) else {
            respond(&mut stream, 400, "잘못된 요청입니다.").await;
            continue;
        };
        if path != "/" {
            respond(&mut stream, 404, "").await;
            continue;
        }
        let get = |k: &str| {
            params
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        if get("state") != Some(state) {
            respond(
                &mut stream,
                400,
                "이 로그인 요청은 clip-ferry가 만든 것이 아닙니다.",
            )
            .await;
            continue;
        }
        if let Some(err) = get("error") {
            respond(
                &mut stream,
                200,
                "로그인하지 않았습니다. clip-ferry로 돌아가 다시 시도하세요.",
            )
            .await;
            return Err(if err == "access_denied" {
                "로그인을 취소했습니다".into()
            } else {
                format!("구글 로그인 오류: {err}")
            });
        }
        match get("code").filter(|c| !c.is_empty()) {
            Some(code) => {
                respond(
                    &mut stream,
                    200,
                    "로그인했습니다. 이 창을 닫고 clip-ferry로 돌아가세요.",
                )
                .await;
                return Ok(code.to_string());
            }
            None => respond(&mut stream, 400, "인가 코드가 없습니다.").await,
        }
    }
}

/// 요청 머리의 첫 줄. 머리 전체가 8 KiB를 넘거나 10초 안에 오지 않으면 `None`.
async fn read_request_line(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let read = async {
        loop {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            if buf.len() > 8 * 1024 {
                return None;
            }
        }
        let head = String::from_utf8_lossy(&buf);
        head.lines().next().map(str::to_string)
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .ok()
        .flatten()
}

async fn respond(stream: &mut TcpStream, status: u16, message: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>clip-ferry</title>\
         <body style=\"font-family:system-ui,sans-serif;padding:40px\"><h2>clip-ferry</h2><p>{message}</p></body>"
    );
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// 시스템 기본 브라우저로 연다.
pub fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.map(|_| ())
        .map_err(|_| "브라우저를 열지 못했습니다".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_example() {
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let fresh = Pkce::new();
        assert_eq!(fresh.verifier.len(), 43);
        assert_ne!(fresh.verifier, Pkce::new().verifier);
    }

    #[test]
    fn auth_url_has_pkce_and_encoded_redirect() {
        let u = auth_url("id.apps", "http://127.0.0.1:5000/", "CH", "ST");
        assert!(u.starts_with("https://accounts.google.com/o/oauth2/v2/auth?client_id=id.apps&"));
        assert!(u.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2F"));
        assert!(u.contains("scope=openid%20email%20profile"));
        assert!(u.contains("code_challenge=CH&code_challenge_method=S256&state=ST"));
    }

    #[test]
    fn parses_callback_query() {
        let (path, q) =
            parse_request_line("GET /?state=a%2Bb&code=4%2F0Ab_c+d&scope=x HTTP/1.1").unwrap();
        assert_eq!(path, "/");
        assert_eq!(q[0], ("state".into(), "a+b".into()));
        assert_eq!(q[1], ("code".into(), "4/0Ab_c d".into()));
        assert!(parse_request_line("POST / HTTP/1.1").is_none());
        assert_eq!(dec("%zz%4"), "%zz%4");
        assert_eq!(dec("%41"), "A");
    }

    async fn browser(port: u16, target: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn loopback_ignores_wrong_state_and_returns_code() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let wait = tokio::spawn(async move { wait_for_code(&listener, "good").await });

        assert!(browser(port, "/favicon.ico")
            .await
            .starts_with("HTTP/1.1 404"));
        let r = browser(port, "/?state=evil&code=stolen").await;
        assert!(r.starts_with("HTTP/1.1 400"));
        let r = browser(port, "/?state=good&code=abc%2F1").await;
        assert!(r.starts_with("HTTP/1.1 200") && r.contains("로그인했습니다"));
        assert_eq!(wait.await.unwrap(), Ok("abc/1".to_string()));
    }

    #[tokio::test]
    async fn loopback_reports_denied() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let wait = tokio::spawn(async move { wait_for_code(&listener, "s").await });
        browser(port, "/?state=s&error=access_denied").await;
        assert_eq!(
            wait.await.unwrap(),
            Err("로그인을 취소했습니다".to_string())
        );
    }

    // ------------------------------------------------------------ 실제 계정 서버로

    use clip_ferry_core::store::MemorySecretStore;
    use std::sync::Arc;

    async fn account_server() -> String {
        use clip_ferry_account::api::{router, AppState};
        use clip_ferry_account::google::{GoogleVerifier, KeySource};
        let state = Arc::new(AppState {
            store: clip_ferry_account::store::Store::memory(),
            signer: clip_ferry_account::tokens::Signer::from_seed(&[3u8; 32]),
            google: GoogleVerifier::new(vec![], KeySource::Static(Default::default())),
            google_desktop: None,
            google_android: None,
            dev_login: true,
            dev_pages: false,
            usage_secret: None,
            admin_emails: vec![],
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn login_refresh_restart_and_logout() {
        let url = account_server().await;
        let store = MemorySecretStore::default();
        let acc = Arc::new(Account::new(Box::new(store.clone())));
        assert!(!acc.is_signed_in());
        assert_eq!(acc.token().await, Err(TokenError::SignedOut));

        let p = acc
            .dev_login(&format!("{url}/"), "Me@Test.dev")
            .await
            .unwrap();
        assert_eq!(p.email, "me@test.dev");
        let first = acc.token().await.unwrap();
        // 보안 저장소에는 갱신 토큰만 있고 접속 토큰은 없다
        let raw = String::from_utf8(store.get(KEY).unwrap()).unwrap();
        assert!(raw.contains("refresh_token") && !raw.contains(&first));

        // 동시에 여러 번 갱신해도 하나씩 하므로 재사용으로 끊기지 않는다
        acc.data.lock().unwrap().access = None;
        let (a, b, c) = tokio::join!(acc.token(), acc.token(), acc.token());
        assert!(a.is_ok() && b.is_ok() && c.is_ok());
        assert_eq!(a, b);
        acc.data.lock().unwrap().access = None;
        acc.token().await.unwrap();

        // 앱을 다시 켜도(같은 저장소) 다시 로그인하지 않는다
        let before_logout = store.get(KEY).unwrap();
        let again = Account::new(Box::new(store.clone()));
        assert!(again.is_signed_in());
        again.token().await.unwrap();
        let dev = "00112233445566778899aabbccddeeff";
        again.register_device(dev, "내 PC").await.unwrap();

        // 로그아웃: 저장소에서 지우고, 서버에서도 그 로그인이 끊긴다
        again.logout().await;
        assert!(!again.is_signed_in() && store.get(KEY).is_none());
        assert_eq!(again.token().await, Err(TokenError::SignedOut));
        let old = MemorySecretStore::default();
        old.set(KEY, &before_logout).unwrap();
        let stale = Account::new(Box::new(old.clone()));
        assert_eq!(stale.token().await, Err(TokenError::SignedOut));
        assert!(old.get(KEY).is_none(), "거절된 갱신 토큰은 지운다");
    }

    /// 계정 연결(F37): 계정 ID와 서버 공개키를 받고, 받은 참여 확인서가 코어 확인을 통과한다.
    /// F34 때 저장한 로그인(계정 ID 없음)도 `/v1/me`로 채운다.
    #[tokio::test]
    async fn account_link_and_ticket() {
        let url = account_server().await;
        let store = MemorySecretStore::default();
        let acc = Account::new(Box::new(store.clone()));
        acc.dev_login(&url, "me@test.dev").await.unwrap();
        // 예전 버전이 저장한 정보처럼 계정 ID를 비운다
        acc.data
            .lock()
            .unwrap()
            .saved
            .as_mut()
            .unwrap()
            .user_id
            .clear();

        let link = acc.account_link().await.unwrap();
        assert_eq!(link.user_id.len(), 32);
        assert_eq!(link.server_keys.len(), 1);
        let raw = String::from_utf8(store.get(KEY).unwrap()).unwrap();
        assert!(raw.contains(&link.user_id), "채운 계정 ID를 저장한다");

        let keys = clip_ferry_core::crypto::DeviceKeys::generate();
        let ticket = acc
            .link_ticket(&keys.sign_public(), &keys.dh_public(), "새 PC")
            .await
            .unwrap();
        let (sign_pub, dh_pub) =
            clip_ferry_core::link::verify_ticket(&ticket, &link, clip_ferry_core::api::now_ms())
                .unwrap();
        assert_eq!(sign_pub, keys.sign_public());
        assert_eq!(dh_pub, keys.dh_public());

        acc.logout().await;
        assert!(acc
            .link_ticket(&keys.sign_public(), &keys.dh_public(), "새 PC")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn account_server_down_is_not_sign_out() {
        let url = account_server().await;
        let store = MemorySecretStore::default();
        let acc = Account::new(Box::new(store.clone()));
        acc.dev_login(&url, "a@b.c").await.unwrap();
        // 계정 서버 주소를 닿지 않는 곳으로 바꿔 둔 것처럼
        acc.data.lock().unwrap().saved.as_mut().unwrap().account_url = "http://127.0.0.1:9".into();
        acc.data.lock().unwrap().access = None;
        assert_eq!(acc.token().await, Err(TokenError::Unavailable));
        assert!(acc.is_signed_in() && store.get(KEY).is_some());
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
