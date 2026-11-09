//! 운영자 통계 (F38, D-023).
//!
//! | 경로 | 하는 일 |
//! |---|---|
//! | `GET /admin` | 운영자 페이지 (구글 로그인 버튼) |
//! | `GET /admin/admin.js` | 그 페이지의 스크립트 |
//! | `POST /admin/api/session` `{id_token}` | 구글 ID 토큰 → 운영자 토큰(1시간). 허용 목록 밖이면 403 |
//! | `GET /admin/api/stats?days=30` | 가입자·DAU·MAU·플랫폼별 기기·일별 합계 |
//! | `GET /admin/api/users?q=&offset=` | 사용자 목록 (이메일·이름 검색, 50개씩) |
//! | `GET /admin/api/users/{id}` | 한 사용자의 계정 정보·기기·사용량(최근 90일) |
//!
//! 허용 목록(`CF_ACCOUNT_ADMIN_EMAILS`)이 비면 이 경로들을 열지 않는다(404).
//! API는 `Authorization: Bearer <운영자 토큰>`을 받는다. 운영자 토큰은 접속 토큰과 대상(aud)이 달라
//! 릴레이·`/v1`에 쓸 수 없고, 접속 토큰도 여기에 쓸 수 없다. 요청마다 허용 목록을 다시 본다.
//! 클립보드 내용은 서버에 없으므로 여기서도 볼 수 없다.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::{ApiError, AppState};
use crate::store::{DayTotal, Device, Overview, UsageDay, User, UserSummary, ADMIN_PAGE};

/// 통계에서 한 번에 볼 수 있는 최대 일수
const MAX_DAYS: i32 = 400;
/// 사용자 상세에서 보여 줄 사용량 일수
const DETAIL_DAYS: i32 = 90;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin", get(page))
        .route("/admin/", get(page))
        .route("/admin/admin.js", get(script))
        .route("/admin/api/session", post(session))
        .route("/admin/api/stats", get(stats))
        .route("/admin/api/users", get(users))
        .route("/admin/api/users/:id", get(user_detail))
}

/// 허용 목록을 정리한다: 앞뒤 공백 제거, 소문자, 빈 값 제거.
pub fn normalize_emails<I, S>(emails: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut v: Vec<String> = emails
        .into_iter()
        .map(|e| e.as_ref().trim().to_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

fn is_admin(s: &AppState, email: &str) -> bool {
    let email = email.trim().to_lowercase();
    s.admin_emails.contains(&email)
}

fn forbidden() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "not_admin")
}

/// `Authorization: Bearer <운영자 토큰>` 확인. 토큰이 없거나 틀리면 401, 허용 목록 밖이면 403.
fn require_admin(s: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let claims = s
        .signer
        .verify_admin(token.trim())
        .map_err(|_| ApiError::unauthorized())?;
    if !is_admin(s, &claims.sub) {
        return Err(forbidden());
    }
    Ok(())
}

fn today() -> i32 {
    (crate::now_ms() / 86_400_000) as i32
}

/// 응답을 캐시에 남기지 않는다 (개인정보).
fn no_store<T: IntoResponse>(body: T) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], body).into_response()
}

// ---------------------------------------------------------------- 페이지

/// 구글 로그인(GIS)에 필요한 곳만 연다. 스크립트는 같은 서버의 파일과 구글 GIS만.
const CSP: &str = "default-src 'self'; \
    script-src 'self' https://accounts.google.com/gsi/client; \
    style-src 'self' 'unsafe-inline' https://accounts.google.com/gsi/style; \
    frame-src https://accounts.google.com/gsi/; \
    connect-src 'self' https://accounts.google.com/gsi/; \
    img-src 'self' data:; \
    base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

async fn page(State(s): State<Arc<AppState>>) -> Response {
    let html = include_str!("admin.html").replace(
        "{{CLIENT_ID}}",
        &crate::api::html_escape(&s.web_client_id().unwrap_or_default()),
    );
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_SECURITY_POLICY, CSP),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        html,
    )
        .into_response()
}

async fn script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("admin.js"),
    )
        .into_response()
}

// ---------------------------------------------------------------- API

#[derive(Deserialize)]
struct SessionBody {
    id_token: String,
}

#[derive(Serialize)]
struct SessionResponse {
    token: String,
    expires_in: i64,
    email: String,
}

async fn session(
    State(s): State<Arc<AppState>>,
    Json(body): Json<SessionBody>,
) -> Result<Response, ApiError> {
    let g = s.google.verify(&body.id_token).await?;
    if !is_admin(&s, &g.email) {
        // 이메일은 로그에 남기지 않는다
        tracing::warn!("admin login refused: not on the allow list");
        return Err(forbidden());
    }
    let email = g.email.trim().to_lowercase();
    let now = crate::now_ms() / 1000;
    let (token, exp) = s.signer.issue_admin(&email, now);
    tracing::info!("admin login");
    Ok(no_store(Json(SessionResponse {
        token,
        expires_in: exp - now,
        email,
    })))
}

#[derive(Deserialize)]
struct StatsQuery {
    days: Option<i32>,
}

#[derive(Serialize)]
struct StatsResponse {
    /// 오늘 (UTC 일 번호)
    today: i32,
    overview: Overview,
    /// 오래된 날부터
    daily: Vec<DayTotal>,
}

async fn stats(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<StatsQuery>,
) -> Result<Response, ApiError> {
    require_admin(&s, &headers)?;
    let today = today();
    let days = q.days.unwrap_or(30).clamp(1, MAX_DAYS);
    let overview = s.store.overview(today, crate::now_ms()).await?;
    let daily = s.store.daily_totals(today - days + 1, today).await?;
    Ok(no_store(Json(StatsResponse {
        today,
        overview,
        daily,
    })))
}

#[derive(Deserialize)]
struct UsersQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: i64,
}

#[derive(Serialize)]
struct UsersResponse {
    total: i64,
    offset: i64,
    limit: i64,
    users: Vec<UserSummary>,
}

async fn users(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<UsersQuery>,
) -> Result<Response, ApiError> {
    require_admin(&s, &headers)?;
    if q.q.chars().count() > 320 {
        return Err(ApiError::bad("bad_query"));
    }
    let offset = q.offset.clamp(0, 1_000_000);
    let (total, users) = s.store.list_users(&q.q, offset, today()).await?;
    Ok(no_store(Json(UsersResponse {
        total,
        offset,
        limit: ADMIN_PAGE,
        users,
    })))
}

#[derive(Serialize)]
struct UserDetail {
    user: User,
    devices: Vec<Device>,
    /// 최근 90일 (날짜 순)
    usage: Vec<UsageDay>,
}

async fn user_detail(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_admin(&s, &headers)?;
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no_user"));
    }
    let user = s
        .store
        .user(&id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no_user"))?;
    let devices = s.store.devices(&id).await?;
    let from = today() - DETAIL_DAYS;
    let usage = s
        .store
        .usage(&id)
        .await?
        .into_iter()
        .filter(|d| d.day > from)
        .collect();
    Ok(no_store(Json(UserDetail {
        user,
        devices,
        usage,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list_is_normalized() {
        assert_eq!(
            normalize_emails([" Op@Example.com ", "", "op@example.com", "b@x.org"]),
            vec!["b@x.org".to_string(), "op@example.com".to_string()]
        );
    }
}
