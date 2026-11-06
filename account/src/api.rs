//! HTTP API.
//!
//! | 경로 | 하는 일 |
//! |---|---|
//! | `GET /healthz` | 살아 있음 |
//! | `GET /v1/keys` | 접속 토큰 공개키(JWKS). 릴레이가 읽는다 |
//! | `POST /v1/auth/google` `{id_token}` | 구글 ID 토큰 → 계정 + 토큰 |
//! | `POST /v1/auth/refresh` `{refresh_token}` | 새 접속·갱신 토큰 (옛 갱신 토큰은 못 쓴다) |
//! | `POST /v1/auth/logout` `{refresh_token}` | 그 로그인의 갱신 토큰 무효화 |
//! | `GET /v1/me` | 내 계정과 기기 (Bearer 접속 토큰) |
//! | `PUT /v1/devices/{device_id}` `{name, platform}` | 기기 등록·갱신 (Bearer) |
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

use crate::google::{GoogleError, GoogleVerifier};
use crate::store::{Device, Identity, Rotate, Store, StoreError, User};
use crate::tokens::{self, Signer};

/// 받을 수 있는 기기 플랫폼 이름
pub const PLATFORMS: [&str; 6] = ["windows", "macos", "linux", "android", "ios", "cli"];

pub struct AppState {
    pub store: Store,
    pub signer: Signer,
    pub google: GoogleVerifier,
    /// `POST /v1/auth/dev` 허용 (개발 전용)
    pub dev_login: bool,
    /// `GET /dev/google` 시험 페이지 (개발 전용)
    pub dev_pages: bool,
}

pub fn router(state: Arc<AppState>) -> Router {
    let mut r = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/keys", get(keys))
        .route("/v1/auth/google", post(auth_google))
        .route("/v1/auth/refresh", post(auth_refresh))
        .route("/v1/auth/logout", post(auth_logout))
        .route("/v1/me", get(me))
        .route("/v1/devices/:device_id", put(put_device));
    if state.dev_login {
        r = r.route("/v1/auth/dev", post(auth_dev));
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
    Ok(Json(MeResponse {
        user: user.into(),
        devices,
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
