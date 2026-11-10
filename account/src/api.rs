//! HTTP API.
//!
//! | 경로 | 하는 일 |
//! |---|---|
//! | `GET /healthz` | 살아 있음 |
//! | `GET /v1/keys` | 접속 토큰 공개키(JWKS). 릴레이가 읽는다 |
//! | `GET /v1/config` | 앱이 쓸 서버 설정 `{relay_url}` (F43. 앱은 계정 서버 주소만 알고 릴레이 주소는 여기서 받는다) |
//! | `POST /v1/auth/google` `{id_token}` | 구글 ID 토큰 → 계정 + 토큰 |
//! | `GET /v1/auth/google/desktop` | 데스크톱 앱이 쓸 구글 클라이언트 ID `{client_id}` (F34) |
//! | `POST /v1/auth/google/code` `{code, code_verifier, redirect_uri}` | 데스크톱 인가 코드 → 계정 + 토큰 (F34, D-020) |
//! | `GET /v1/auth/google/android` | Android 앱이 Credential Manager에 넘길 웹 클라이언트 ID `{client_id}` (F35) |
//! | `POST /v1/auth/refresh` `{refresh_token}` | 새 접속·갱신 토큰 (옛 갱신 토큰은 못 쓴다) |
//! | `POST /v1/auth/logout` `{refresh_token}` | 그 로그인의 갱신 토큰 무효화 |
//! | `GET /v1/me` | 내 계정과 기기 (Bearer 접속 토큰) |
//! | `DELETE /v1/me` | 계정 삭제: 계정·기기·갱신 토큰·사용량을 바로 지운다 (Bearer, F39) |
//! | `PUT /v1/devices/{device_id}` `{name, platform}` | 기기 등록·갱신 (Bearer) |
//! | `POST /v1/link/ticket` `{sign_pub, dh_pub, name}` | 계정 연결 참여 확인서 (Bearer, F37) |
//! | `POST /internal/usage` `{entries: [{user_id, messages, bytes}]}` | 릴레이가 보내는 사용량 (Bearer 공유 비밀, F33) |
//! | `POST /v1/auth/dev` `{email, name}` | 개발용 로그인 (`--dev-login`일 때만) |
//! | `GET /dev/google` | 구글 로그인 시험 페이지 (`--dev-pages`일 때만) |
//! | `/admin…` | 운영자 통계 (F38, `admin` 모듈. 허용 목록이 있을 때만) |
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
    /// 운영자 구글 계정 이메일 허용 목록(소문자, F38). 비면 `/admin`을 열지 않는다.
    /// `admin::normalize_emails`로 만든다.
    pub admin_emails: Vec<String>,
    /// 앱에 알려 줄 릴레이 주소(`ws://`·`wss://`, F43). 없으면 `/v1/config`의 `relay_url`이 `null`이고
    /// 앱은 저장해 둔 값이나 내장 기본값을 쓴다.
    pub relay_url: Option<String>,
}

impl AppState {
    /// 웹 페이지의 구글 로그인 버튼에 쓸 "웹 애플리케이션" 클라이언트 ID.
    pub fn web_client_id(&self) -> Option<String> {
        self.google_android
            .clone()
            .or_else(|| self.google.client_ids().first().cloned())
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let mut r = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/keys", get(keys))
        .route("/v1/config", get(app_config))
        .route("/v1/auth/google", post(auth_google))
        .route("/v1/auth/google/desktop", get(google_desktop_config))
        .route("/v1/auth/google/code", post(auth_google_code))
        .route("/v1/auth/google/android", get(google_android_config))
        .route("/v1/auth/refresh", post(auth_refresh))
        .route("/v1/auth/logout", post(auth_logout))
        .route("/v1/me", get(me).delete(delete_me))
        .route("/v1/devices/:device_id", put(put_device))
        .route("/v1/link/ticket", post(link_ticket))
        .route("/v1/group/restart", post(restart_group));
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
    if !state.admin_emails.is_empty() {
        r = r.merge(crate::admin::routes());
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
    pub(crate) fn new(status: StatusCode, code: &'static str) -> Self {
        ApiError { status, code }
    }
    pub(crate) fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
    pub(crate) fn bad(code: &'static str) -> Self {
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
            StoreError::Retired => Self::new(StatusCode::CONFLICT, "device_retired"),
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

/// 앱이 로그인 전에도 읽는 서버 설정 (F43). 비밀은 없다.
async fn app_config(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "relay_url": s.relay_url }))
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

/// 계정 삭제(F39). 이 계정의 모든 기기에서 로그인이 풀린다(갱신 토큰이 없어진다).
/// 이미 받은 접속 토큰은 만료(최대 1시간)까지 릴레이 접속에 쓰일 수 있다(D-024).
/// 같은 요청이 두 번 와도 204다.
async fn delete_me(State(s): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<StatusCode> {
    let user_id = bearer_user(&s, &headers)?;
    if s.store.delete_user(&user_id).await? {
        tracing::info!("account deleted");
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct DeviceBody {
    name: String,
    platform: String,
    /// 이 기기가 그룹에 있는가 (F44). 없으면 바꾸지 않는다(예전 앱)
    #[serde(default)]
    in_group: Option<bool>,
}

fn valid_device_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

async fn put_device(
    State(s): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<DeviceBody>,
) -> ApiResult<Json<Device>> {
    let user_id = bearer_user(&s, &headers)?;
    if !valid_device_id(&device_id) {
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
        .upsert_device(
            &user_id,
            &device_id,
            name,
            &body.platform,
            body.in_group,
            crate::now_ms(),
        )
        .await?;
    Ok(Json(d))
}

#[derive(Deserialize)]
struct RestartBody {
    device_id: String,
}

/// 처음부터 다시 시작(F44, D-027): 이 계정의 다른 기기를 예전 그룹으로 표시한다.
/// 그 기기들은 다음 등록 때 `409 device_retired`를 받고 그룹을 떠난다.
async fn restart_group(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RestartBody>,
) -> ApiResult<StatusCode> {
    let user_id = bearer_user(&s, &headers)?;
    if !valid_device_id(&body.device_id) {
        return Err(ApiError::bad("bad_device_id"));
    }
    let n = s.store.restart_group(&user_id, &body.device_id).await?;
    tracing::info!(retired = n, "group restarted");
    Ok(StatusCode::NO_CONTENT)
}

/// 계정 연결(F37): 이 계정에 로그인한 새 기기의 공개키에 서명해 준다.
async fn link_ticket(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<crate::link::TicketRequest>,
) -> ApiResult<Json<crate::link::Ticket>> {
    let user_id = bearer_user(&s, &headers)?;
    // 지워진 계정의 토큰이 아직 살아 있어도 확인서는 주지 않는다
    s.store
        .user(&user_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    crate::link::issue(&s.signer, &user_id, &body, crate::now_ms())
        .map(Json)
        .map_err(|e| match e {
            crate::link::TicketError::BadKey => ApiError::bad("bad_key"),
            crate::link::TicketError::BadName => ApiError::bad("bad_name"),
        })
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

pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
