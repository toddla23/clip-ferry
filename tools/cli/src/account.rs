//! 개발용 계정 로그인 (F33). 계정 서버의 `POST /v1/auth/dev`(`--dev-login`)로 토큰을 받는다.
//!
//! D-010과 같은 **개발 전용** 동작이다: 토큰을 프로필 폴더의 `account.json`에 평문으로 둔다.
//! 앱(셸)은 이렇게 하지 않는다(F34~F36에서 플랫폼 보안 저장소에 둔다).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use clip_ferry_core::transport::{TokenSource, TransportConfig};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// 만료까지 이보다 적게 남았으면 갱신한다
const REFRESH_MARGIN_SECS: i64 = 120;

#[derive(Serialize, Deserialize, Clone)]
struct Saved {
    account_url: String,
    access_token: String,
    /// 접속 토큰 만료 (Unix 초)
    access_expires_at: i64,
    refresh_token: String,
    email: String,
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
    email: String,
}

fn file(dir: &Path) -> PathBuf {
    dir.join("account.json")
}

fn load(dir: &Path) -> Option<Saved> {
    let text = std::fs::read_to_string(file(dir)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(dir: &Path, saved: &Saved) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(file(dir), serde_json::to_vec_pretty(saved)?)?;
    Ok(())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("HTTP 클라이언트")
}

fn from_response(account_url: &str, r: TokenResponse) -> Saved {
    Saved {
        account_url: account_url.trim_end_matches('/').to_string(),
        access_token: r.access_token,
        access_expires_at: now_secs() + r.expires_in,
        refresh_token: r.refresh_token,
        email: r.user.email,
    }
}

/// `cf login --dev`
pub async fn login_dev(
    dir: &Path,
    account_url: &str,
    email: &str,
    name: &str,
) -> anyhow::Result<()> {
    let base = account_url.trim_end_matches('/');
    let resp = http()
        .post(format!("{base}/v1/auth/dev"))
        .json(&serde_json::json!({ "email": email, "name": name }))
        .send()
        .await
        .with_context(|| format!("계정 서버({base})에 연결하지 못했습니다"))?;
    match resp.status().as_u16() {
        200 => {}
        404 => bail!("계정 서버가 개발용 로그인을 받지 않습니다 (--dev-login으로 띄운다)"),
        s => bail!("로그인 실패: HTTP {s}"),
    }
    let saved = from_response(base, resp.json().await?);
    save(dir, &saved)?;
    eprintln!("경고: 개발용 로그인입니다. 토큰이 프로필 폴더(account.json)에 평문으로 저장됩니다.");
    println!("로그인됨: {}", saved.email);
    Ok(())
}

/// `cf account`: 계정 서버의 `/v1/me`
pub async fn show(dir: &Path) -> anyhow::Result<()> {
    let lock = Arc::new(Mutex::new(()));
    let Some(token) = fresh_token(dir.to_path_buf(), lock).await else {
        bail!("로그인하지 않았습니다 (cf login --dev --email …)");
    };
    let saved = load(dir).context("account.json")?;
    let me: serde_json::Value = http()
        .get(format!("{}/v1/me", saved.account_url))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!("{}", serde_json::to_string_pretty(&me)?);
    Ok(())
}

/// `cf logout`: 갱신 토큰을 무효화하고 파일을 지운다.
pub async fn logout(dir: &Path) -> anyhow::Result<()> {
    if let Some(saved) = load(dir) {
        let _ = http()
            .post(format!("{}/v1/auth/logout", saved.account_url))
            .json(&serde_json::json!({ "refresh_token": saved.refresh_token }))
            .send()
            .await;
        std::fs::remove_file(file(dir))?;
    }
    println!("로그아웃됨");
    Ok(())
}

/// 쓸 수 있는 접속 토큰. 만료가 가까우면 갱신한다(한 번에 하나씩 — 같은 갱신 토큰을 두 번 쓰면
/// 계정 서버가 재사용으로 보고 모든 로그인을 끊는다).
async fn fresh_token(dir: PathBuf, lock: Arc<Mutex<()>>) -> Option<String> {
    let _one = lock.lock().await;
    let saved = load(&dir)?;
    if saved.access_expires_at - now_secs() > REFRESH_MARGIN_SECS {
        return Some(saved.access_token);
    }
    let resp = http()
        .post(format!("{}/v1/auth/refresh", saved.account_url))
        .json(&serde_json::json!({ "refresh_token": saved.refresh_token }))
        .send()
        .await
        .ok()?;
    if resp.status().as_u16() == 401 {
        eprintln!("로그인이 만료되었습니다. cf login --dev로 다시 로그인하세요.");
        return None;
    }
    let r: TokenResponse = resp.error_for_status().ok()?.json().await.ok()?;
    let next = from_response(&saved.account_url, r);
    let token = next.access_token.clone();
    if let Err(e) = save(&dir, &next) {
        eprintln!("account.json을 저장하지 못했습니다: {e}");
    }
    Some(token)
}

static CONFIG: OnceLock<TransportConfig> = OnceLock::new();

/// 프로필에 로그인 정보가 있으면 릴레이 연결에 접속 토큰을 싣는다.
pub fn init(dir: &Path) {
    let config = if load(dir).is_some() {
        let dir = dir.to_path_buf();
        let lock = Arc::new(Mutex::new(()));
        TransportConfig {
            token: Some(TokenSource::new(move || {
                fresh_token(dir.clone(), lock.clone())
            })),
            ..TransportConfig::default()
        }
    } else {
        TransportConfig::default()
    };
    let _ = CONFIG.set(config);
}

/// 이 CLI의 모든 릴레이 연결이 쓰는 설정
pub fn transport_config() -> TransportConfig {
    CONFIG.get().cloned().unwrap_or_default()
}
