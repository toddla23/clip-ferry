//! HTTP API.
//!
//! | 경로 | 하는 일 |
//! |---|---|
//! | `GET /healthz` | 살아 있음 |
//! | `GET /v1/keys` | 접속 토큰 공개키(JWKS). 릴레이가 읽는다 |
//! | `POST /v1/auth/google` `{id_token}` | 구글 ID 토큰 → 계정 + 토큰 |
//! | `GET /v1/auth/google/desktop` | 데스크톱 앱이 쓸 구글 클라이언트 ID `{client_id}` (F34) |
//! | `POST /v1/auth/google/code` `{code, code_verifier, redirect_uri}` | 데스크톱 인가 코드 → 계정 + 토큰 (F34, D-020) |
//! | `GET /v1/auth/google/android` | Android 앱이 Credential Manager에 넘길 웹 클라이언트 ID `{client_id}` (F35) |
//! | `POST /v1/auth/refresh` `{refresh_token}` | 새 접속·갱신 토큰 (옛 갱신 토큰은 못 쓴다) |
//! | `POST /v1/auth/logout` `{refresh_token}` | 그 로그인의 갱신 토큰 무효화 |
//! | `GET /v1/me` | 내 계정과 기기 (Bearer 접속 토큰) |
//! | `PUT /v1/devices/{device_id}` `{name, platform}` | 기기 등록·갱신 (Bearer) |
//! | `POST /internal/usage` `{entries: [{user_id, messages, bytes}]}` | 릴레이가 보내는 사용량 (Bearer 공유 비밀, F33) |
//! | `POST /v1/auth/dev` `{email, name}` | 개발용 로그인 (`--dev-login`일 때만) |
//! | `GET /dev/google` | 구글 로그인 시험 페이지 (`--dev-pages`일 때만) |
//!
//! 오류 응답은 `{"error": "<코드>"}`. 토큰·이메일은 로그에 남기지 않는다.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::google::{is_loopback_redirect, CodeError, CodeExchange, GoogleError, GoogleVerifier};
use crate::store::{Device, Identity, Rotate, Store, StoreError, UsageDay, User};
use crate::tokens::{self, Signer};

/// 받을 수 있는 기기 플랫폼 이름
pub const PLATFORMS: [&str; 6] = ["windows", "macos", "linux", "android", "ios", "cli"];

pub struct AppState {
    pub store: Store,
    pub signer: Signer,
    pub google: GoogleVerifier,
    /// 데스크톱 앱 구글 로그인(인가 코드 교환). 없으면 데스크톱 로그인 경로를 열지 않는다.
    /// 클라이언트 ID는 `google`의 허용 목록에도 있어야 한다.
    pub google_desktop: Option<CodeExchange>,
    /// Android 앱이 `serverClientId`로 쓸 웹 클라이언트 ID (F35). 받은 ID 토큰의 `aud`가 이 값이다.
    /// `google`의 허용 목록에 있어야 한다. 없으면 Android 로그인 경로를 열지 않는다.
    pub google_android: Option<String>,
    /// `POST /v1/auth/dev` 허용 (개발 전용)
    pub dev_login: bool,
    /// `GET /dev/google` 시험 페이지 (개발 전용)
    pub dev_pages: bool,
    /// 릴레이 사용량 보고(`POST /internal/usage`)의 공유 비밀. 없으면 그 경로를 열지 않는다.
    pub usage_secret: Option<String>,
}

pub fn router(state: Arc<AppState>) -> Router {
    let mut r = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/keys", get(keys))
        .route("/v1/auth/google", post(auth_google))
        .route("/v1/auth/google/desktop", get(google_desktop_config))
        .route("/v1/auth/google/code", post(auth_google_code))
        .route("/v1/auth/google/android", get(google_android_config))
        .route("/v1/auth/refresh", post(auth_refresh))
        .route("/v1/auth/logout", post(auth_logout))
        .route("/v1/me", get(me))
        .route("/v1/devices/:device_id", put(put_device));
    if state.dev_login {
        r = r.route("/v1/auth/dev", post(auth_dev));
    }
    if state.usage_secret.is_some() {
        // 릴레이가 계정 여러 개를 한 번에 보내므로 본문 한도를 따로 둔다
        r = r.route(
            "/internal/usage",
            post(internal_usage).layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        );
    }
    if state.dev_pages {
        r = r.route("/dev/google", get(dev_google_page));
    }
    r.layer(DefaultBodyLimit::max(16 * 1024)).with_state(state)
}

// ---------------------------------------------------------------- 오류

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str) -> Self {
        ApiError { status, code }
    }
    fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
    fn bad(code: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.code }))).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::TooManyDevices => Self::new(StatusCode::CONFLICT, "too_many_devices"),
            StoreError::NoUser => Self::unauthorized(),
            e => {
                tracing::error!("store: {e}");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        }
    }
}

impl From<GoogleError> for ApiError {
    fn from(e: GoogleError) -> Self {
        match e {
            GoogleError::Invalid(_) => Self::new(StatusCode::UNAUTHORIZED, "invalid_id_token"),
            GoogleError::EmailNotVerified => Self::new(StatusCode::FORBIDDEN, "email_not_verified"),
            GoogleError::KeysUnavailable => {
                Self::new(StatusCode::SERVICE_UNAVAILABLE, "google_unavailable")
            }
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ---------------------------------------------------------------- 응답 모양

#[derive(Serialize)]
pub struct UserView {
    pub id: String,
    pub provider: String,
    pub email: String,
    pub name: String,
    pub picture_url: String,
    pub created_at_ms: i64,
}

impl From<User> for UserView {
    fn from(u: User) -> Self {
        UserView {
            id: u.id,
            provider: u.provider,
            email: u.email,
            name: u.name,
            picture_url: u.picture_url,
            created_at_ms: u.created_at_ms,
        }
    }
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    /// 접속 토큰 만료까지 남은 초
    pub expires_in: i64,
    pub refresh_token: String,
    pub user: UserView,
}

#[derive(Serialize)]
pub struct MeResponse {
    pub user: UserView,
    pub devices: Vec<Device>,
    /// 일별 사용량 (릴레이가 보낸 게시 수·바이트, 내용 없음). 최근 30일
    pub usage: Vec<UsageDay>,
}

// ---------------------------------------------------------------- 처리기

async fn keys(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "public, max-age=300")],
        Json(s.signer.jwks()),
    )
}

#[derive(Deserialize)]
struct GoogleLogin {
    id_token: String,
}

async fn auth_google(
    State(s): State<Arc<AppState>>,
    Json(body): Json<GoogleLogin>,
) -> ApiResult<Json<TokenResponse>> {
    let g = s.google.verify(&body.id_token).await?;
    let who = Identity {
        provider: "google".into(),
        sub: g.sub,
        email: g.email,
        name: g.name,
        picture_url: g.picture_url,
    };
    login(&s, &who).await
}

async fn google_desktop_config(
    State(s): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let ex = s
        .google_desktop
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "desktop_login_disabled"))?;
    Ok(Json(serde_json::json!({ "client_id": ex.client_id() })))
}

async fn google_android_config(
    State(s): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = s
        .google_android
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "android_login_disabled"))?;
    Ok(Json(serde_json::json!({ "client_id": id })))
}

#[derive(Deserialize)]
struct CodeLogin {
    code: String,
    code_verifier: String,
    redirect_uri: String,
}

/// 데스크톱 앱: 루프백으로 받은 인가 코드를 구글에서 ID 토큰으로 바꾸고, 그 토큰을 검증해 로그인한다.
async fn auth_google_code(
    State(s): State<Arc<AppState>>,
    Json(body): Json<CodeLogin>,
) -> ApiResult<Json<TokenResponse>> {
    let ex = s
        .google_desktop
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "desktop_login_disabled"))?;
    if !is_loopback_redirect(&body.redirect_uri) {
        return Err(ApiError::bad("bad_redirect_uri"));
    }
    // RFC 7636: 43~128자
    let v = &body.code_verifier;
    if !(43..=128).contains(&v.len()) || body.code.is_empty() || body.code.len() > 2048 {
        return Err(ApiError::bad("bad_code"));
    }
    let id_token = ex
        .exchange(&body.code, v, &body.redirect_uri)
        .await
        .map_err(|e| match e {
            CodeError::Rejected => ApiError::new(StatusCode::UNAUTHORIZED, "invalid_code"),
            CodeError::Unavailable => {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "google_unavailable")
            }
        })?;
    let g = s.google.verify(&id_token).await?;
    let who = Identity {
        provider: "google".into(),
        sub: g.sub,
        email: g.email,
        name: g.name,
        picture_url: g.picture_url,
    };
    login(&s, &who).await
}

#[derive(Deserialize)]
struct DevLogin {
    email: String,
    #[serde(default)]
    name: String,
}

async fn auth_dev(
    State(s): State<Arc<AppState>>,
    Json(body): Json<DevLogin>,
) -> ApiResult<Json<TokenResponse>> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() || email.len() > 320 || !email.contains('@') {
        return Err(ApiError::bad("bad_email"));
    }
    let who = Identity {
        provider: "dev".into(),
        sub: email.clone(),
        email,
        name: body.name,
        picture_url: String::new(),
    };
    login(&s, &who).await
}

/// 계정을 만들거나 갱신하고, 새 로그인 묶음으로 토큰을 준다.
async fn login(s: &AppState, who: &Identity) -> ApiResult<Json<TokenResponse>> {
    let now = crate::now_ms();
    let user = s.store.upsert_user(who, now).await?;
    let (refresh, hash) = tokens::new_refresh_token();
    s.store
        .insert_refresh(
            &hash,
            &user.id,
            &crate::random_id(),
            now,
            now + tokens::REFRESH_TTL_MS,
        )
        .await?;
    Ok(Json(token_response(s, user, refresh, now)))
}

fn token_response(s: &AppState, user: User, refresh_token: String, now_ms: i64) -> TokenResponse {
    let now = now_ms / 1000;
    let (access_token, exp) = s.signer.issue(&user.id, now);
    TokenResponse {
        access_token,
        token_type: "Bearer",
        expires_in: exp - now,
        refresh_token,
        user: user.into(),
    }
}

#[derive(Deserialize)]
struct RefreshBody {
    refresh_token: String,
}

async fn auth_refresh(
    State(s): State<Arc<AppState>>,
    Json(body): Json<RefreshBody>,
) -> ApiResult<Json<TokenResponse>> {
    let now = crate::now_ms();
    let old = tokens::refresh_hash(&body.refresh_token);
    let (refresh, new) = tokens::new_refresh_token();
    match s
        .store
        .rotate_refresh(&old, &new, now, now + tokens::REFRESH_TTL_MS)
        .await?
    {
        Rotate::Rotated { user_id } => {
            let user = s
                .store
                .user(&user_id)
                .await?
                .ok_or_else(ApiError::unauthorized)?;
            Ok(Json(token_response(&s, user, refresh, now)))
        }
        Rotate::Reused => {
            tracing::warn!("refresh token reuse detected; revoked account sessions");
            Err(ApiError::new(StatusCode::UNAUTHORIZED, "refresh_reused"))
        }
        Rotate::Invalid => Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid_refresh")),
    }
}

async fn auth_logout(
    State(s): State<Arc<AppState>>,
    Json(body): Json<RefreshBody>,
) -> ApiResult<StatusCode> {
    s.store
        .revoke_family_of(&tokens::refresh_hash(&body.refresh_token))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `Authorization: Bearer <접속 토큰>` → 계정 ID
fn bearer_user(s: &AppState, headers: &HeaderMap) -> ApiResult<String> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(ApiError::unauthorized)?;
    let token = value
        .strip_prefix("Bearer ")
        .ok_or_else(ApiError::unauthorized)?;
    s.signer
        .verify(token.trim())
        .map(|c| c.sub)
        .map_err(|_| ApiError::unauthorized())
}

async fn me(State(s): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<MeResponse>> {
    let user_id = bearer_user(&s, &headers)?;
    let user = s
        .store
        .user(&user_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let devices = s.store.devices(&user_id).await?;
    let today = (crate::now_ms() / 86_400_000) as i32;
    let usage = s
        .store
        .usage(&user_id)
        .await?
        .into_iter()
        .filter(|d| d.day > today - 30)
        .collect();
    Ok(Json(MeResponse {
        user: user.into(),
        devices,
        usage,
    }))
}

#[derive(Deserialize)]
struct DeviceBody {
    name: String,
    platform: String,
}

async fn put_device(
    State(s): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<DeviceBody>,
) -> ApiResult<Json<Device>> {
    let user_id = bearer_user(&s, &headers)?;
    if device_id.len() != 32
        || !device_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ApiError::bad("bad_device_id"));
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(ApiError::bad("bad_name"));
    }
    if !PLATFORMS.contains(&body.platform.as_str()) {
        return Err(ApiError::bad("bad_platform"));
    }
    let d = s
        .store
        .upsert_device(&user_id, &device_id, name, &body.platform, crate::now_ms())
        .await?;
    Ok(Json(d))
}

#[derive(Deserialize)]
struct UsageBody {
    entries: Vec<UsageItem>,
}

#[derive(Deserialize)]
struct UsageItem {
    user_id: String,
    messages: u64,
    bytes: u64,
}

/// 한 번에 받을 계정 수
const MAX_USAGE_ENTRIES: usize = 20_000;

async fn internal_usage(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UsageBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let expected = s
        .usage_secret
        .as_deref()
        .ok_or_else(ApiError::unauthorized)?;
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    // 길이와 내용이 새지 않게 해시끼리 비교한다
    use sha2::{Digest, Sha256};
    if Sha256::digest(given.trim().as_bytes()) != Sha256::digest(expected.as_bytes()) {
        return Err(ApiError::unauthorized());
    }
    if body.entries.len() > MAX_USAGE_ENTRIES {
        return Err(ApiError::bad("too_many_entries"));
    }
    let entries: Vec<(String, i64, i64)> = body
        .entries
        .into_iter()
        .filter(|e| e.user_id.len() == 32)
        .map(|e| {
            (
                e.user_id,
                e.messages.min(i64::MAX as u64) as i64,
                e.bytes.min(i64::MAX as u64) as i64,
            )
        })
        .collect();
    let day = (crate::now_ms() / 86_400_000) as i32;
    let accepted = s.store.add_usage(day, &entries).await?;
    Ok(Json(serde_json::json!({ "accepted": accepted })))
}

async fn dev_google_page(State(s): State<Arc<AppState>>) -> Html<String> {
    let client_id = s.google.client_ids().first().cloned().unwrap_or_default();
    Html(include_str!("dev_google.html").replace("{{CLIENT_ID}}", &html_escape(&client_id)))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
