//! 계정 저장소. 운영은 MariaDB, 테스트·개발은 메모리.
//!
//! 두 구현은 같은 동작을 해야 한다(통합 테스트가 둘 다 돌린다. MariaDB는 `CF_TEST_MARIADB_URL`이 있을 때).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::Row;

/// 계정당 기기 수 한도
pub const MAX_DEVICES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub id: String,
    pub provider: String,
    #[serde(skip)]
    pub provider_sub: String,
    pub email: String,
    pub name: String,
    pub picture_url: String,
    pub created_at_ms: i64,
    pub last_login_ms: i64,
}

/// 로그인 제공자가 확인해 준 신원.
#[derive(Debug, Clone)]
pub struct Identity {
    pub provider: String,
    pub sub: String,
    pub email: String,
    pub name: String,
    pub picture_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub created_at_ms: i64,
    pub last_seen_ms: i64,
}

/// 하루 사용량 (릴레이가 보낸 게시 수·바이트).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageDay {
    /// Unix 시간 기준 일 번호 (UTC)
    pub day: i32,
    pub messages: i64,
    pub bytes: i64,
}

/// 운영자 통계(F38): 플랫폼별 기기 수.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformCount {
    pub platform: String,
    /// 등록된 기기 (지운 기기도 계정 서버에는 남는다)
    pub devices: i64,
    /// 그중 최근 30일 안에 접속한 기기
    pub seen_30d: i64,
}

/// 운영자 통계: 하루 합계. 활성 = 그날 한 번이라도 전송한 계정(D-023).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DayTotal {
    pub day: i32,
    pub active_users: i64,
    pub new_users: i64,
    pub messages: i64,
    pub bytes: i64,
}

/// 운영자 통계: 전체 요약.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overview {
    pub users: i64,
    /// 오늘(UTC) 전송한 계정 수
    pub dau: i64,
    /// 오늘 포함 최근 30일 안에 전송한 계정 수
    pub mau: i64,
    pub platforms: Vec<PlatformCount>,
}

/// 운영자 사용자 목록의 한 줄.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserSummary {
    #[serde(flatten)]
    pub user: User,
    pub device_count: i64,
    /// 오늘 포함 최근 30일
    pub messages_30d: i64,
    pub bytes_30d: i64,
    /// 마지막으로 전송한 날 (없으면 null)
    pub last_active_day: Option<i32>,
}

/// 운영자 목록 한 쪽의 최대 줄 수
pub const ADMIN_PAGE: i64 = 50;
const DAY_MS: i64 = 86_400_000;

/// 갱신 토큰 교체 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rotate {
    /// 새 토큰으로 바꿨다
    Rotated { user_id: String },
    /// 이미 교체에 쓰인 토큰이 다시 왔다 → 그 계정의 갱신 토큰을 모두 무효화했다
    Reused,
    /// 없거나 만료되었거나 무효화된 토큰
    Invalid,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("계정에 기기가 너무 많습니다")]
    TooManyDevices,
    #[error("계정이 없습니다")]
    NoUser,
    #[error("DB 오류: {0}")]
    Db(#[from] sqlx::Error),
    #[error("마이그레이션 오류: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub enum Store {
    Memory(Box<MemStore>),
    Maria(MariaStore),
}

impl Store {
    pub fn memory() -> Self {
        Store::Memory(Box::default())
    }

    /// MariaDB에 연결하고 마이그레이션을 적용한다.
    pub async fn connect_mariadb(url: &str) -> Result<Self> {
        let pool = MySqlPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Store::Maria(MariaStore { pool }))
    }

    /// 로그인: 신원으로 계정을 찾아 정보를 갱신하거나 새로 만든다.
    pub async fn upsert_user(&self, who: &Identity, now: i64) -> Result<User> {
        let who = Identity {
            provider: who.provider.clone(),
            sub: who.sub.clone(),
            email: clip(&who.email, 320),
            name: clip(&who.name, 255),
            picture_url: clip(&who.picture_url, 2048),
        };
        match self {
            Store::Memory(m) => Ok(m.upsert_user(&who, now)),
            Store::Maria(m) => m.upsert_user(&who, now).await,
        }
    }

    pub async fn user(&self, id: &str) -> Result<Option<User>> {
        match self {
            Store::Memory(m) => Ok(m.lock().users.get(id).cloned()),
            Store::Maria(m) => m.user(id).await,
        }
    }

    /// 계정의 기기 (등록 순)
    pub async fn devices(&self, user_id: &str) -> Result<Vec<Device>> {
        match self {
            Store::Memory(m) => Ok(m.devices(user_id)),
            Store::Maria(m) => m.devices(user_id).await,
        }
    }

    /// 기기를 등록하거나 이름·플랫폼·마지막 접속 시각을 갱신한다.
    pub async fn upsert_device(
        &self,
        user_id: &str,
        device_id: &str,
        name: &str,
        platform: &str,
        now: i64,
    ) -> Result<Device> {
        let name = clip(name, 255);
        match self {
            Store::Memory(m) => m.upsert_device(user_id, device_id, &name, platform, now),
            Store::Maria(m) => {
                m.upsert_device(user_id, device_id, &name, platform, now)
                    .await
            }
        }
    }

    pub async fn insert_refresh(
        &self,
        hash: &[u8; 32],
        user_id: &str,
        family: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<()> {
        match self {
            Store::Memory(m) => {
                m.lock().refresh.insert(
                    *hash,
                    MemRefresh {
                        user_id: user_id.into(),
                        family: family.into(),
                        expires_at,
                        used_at: None,
                        revoked: false,
                    },
                );
                Ok(())
            }
            Store::Maria(m) => {
                m.insert_refresh(hash, user_id, family, now, expires_at)
                    .await
            }
        }
    }

    /// `old`를 교체에 쓴 것으로 표시하고 같은 묶음의 `new`를 넣는다.
    pub async fn rotate_refresh(
        &self,
        old: &[u8; 32],
        new: &[u8; 32],
        now: i64,
        new_expires_at: i64,
    ) -> Result<Rotate> {
        match self {
            Store::Memory(m) => Ok(m.rotate_refresh(old, new, now, new_expires_at)),
            Store::Maria(m) => m.rotate_refresh(old, new, now, new_expires_at).await,
        }
    }

    /// 로그아웃: 이 토큰이 속한 묶음(한 번의 로그인)을 무효화한다. 없는 토큰이면 아무것도 안 한다.
    pub async fn revoke_family_of(&self, hash: &[u8; 32]) -> Result<()> {
        match self {
            Store::Memory(m) => {
                let mut g = m.lock();
                if let Some(family) = g.refresh.get(hash).map(|r| r.family.clone()) {
                    for r in g.refresh.values_mut().filter(|r| r.family == family) {
                        r.revoked = true;
                    }
                }
                Ok(())
            }
            Store::Maria(m) => m.revoke_family_of(hash).await,
        }
    }

    /// 릴레이가 보낸 사용량을 `day`에 더한다. 없는 계정은 건너뛴다. 더한 계정 수를 돌려준다.
    pub async fn add_usage(&self, day: i32, entries: &[(String, i64, i64)]) -> Result<u64> {
        match self {
            Store::Memory(m) => {
                let mut g = m.lock();
                let mut n = 0;
                for (user, messages, bytes) in entries {
                    if !g.users.contains_key(user) {
                        continue;
                    }
                    let e = g.usage.entry((user.clone(), day)).or_insert((0, 0));
                    e.0 += messages;
                    e.1 += bytes;
                    n += 1;
                }
                Ok(n)
            }
            Store::Maria(m) => {
                let mut n = 0;
                for (user, messages, bytes) in entries {
                    // 없는 계정이면 SELECT가 비어 아무것도 넣지 않는다 (외래 키 오류를 피한다)
                    let r = sqlx::query(
                        "INSERT INTO usage_daily (user_id, day, messages, bytes) \
                         SELECT id, ?, ?, ? FROM users WHERE id = ? \
                         ON DUPLICATE KEY UPDATE messages = messages + VALUES(messages), \
                         bytes = bytes + VALUES(bytes)",
                    )
                    .bind(day)
                    .bind(messages)
                    .bind(bytes)
                    .bind(user)
                    .execute(&m.pool)
                    .await?;
                    if r.rows_affected() > 0 {
                        n += 1;
                    }
                }
                Ok(n)
            }
        }
    }

    /// 계정의 일별 사용량 (날짜 순)
    pub async fn usage(&self, user_id: &str) -> Result<Vec<UsageDay>> {
        match self {
            Store::Memory(m) => {
                let g = m.lock();
                let mut v: Vec<UsageDay> = g
                    .usage
                    .iter()
                    .filter(|((u, _), _)| u == user_id)
                    .map(|((_, day), (messages, bytes))| UsageDay {
                        day: *day,
                        messages: *messages,
                        bytes: *bytes,
                    })
                    .collect();
                v.sort_by_key(|d| d.day);
                Ok(v)
            }
            Store::Maria(m) => {
                let rows = sqlx::query(
                    "SELECT day, messages, bytes FROM usage_daily WHERE user_id = ? ORDER BY day",
                )
                .bind(user_id)
                .fetch_all(&m.pool)
                .await?;
                rows.iter()
                    .map(|r| {
                        Ok(UsageDay {
                            day: r.try_get("day")?,
                            messages: r.try_get("messages")?,
                            bytes: r.try_get("bytes")?,
                        })
                    })
                    .collect()
            }
        }
    }

    /// 만료된 지 하루가 지난 갱신 토큰을 지운다. 지운 수를 돌려준다.
    pub async fn prune(&self, now: i64) -> Result<u64> {
        let cutoff = now - 24 * 60 * 60 * 1000;
        match self {
            Store::Memory(m) => {
                let mut g = m.lock();
                let before = g.refresh.len();
                g.refresh.retain(|_, r| r.expires_at >= cutoff);
                Ok((before - g.refresh.len()) as u64)
            }
            Store::Maria(m) => {
                let r = sqlx::query("DELETE FROM refresh_tokens WHERE expires_at_ms < ?")
                    .bind(cutoff)
                    .execute(&m.pool)
                    .await?;
                Ok(r.rows_affected())
            }
        }
    }

    /// 운영자 요약. `today`는 UTC 일 번호, `now`는 밀리초.
    pub async fn overview(&self, today: i32, now: i64) -> Result<Overview> {
        let recent = now - 30 * DAY_MS;
        match self {
            Store::Memory(m) => Ok(m.overview(today, recent)),
            Store::Maria(m) => m.overview(today, recent).await,
        }
    }

    /// `from`~`to`(포함) 일별 합계. 빈 날도 0으로 채워 날짜 순으로 돌려준다.
    pub async fn daily_totals(&self, from: i32, to: i32) -> Result<Vec<DayTotal>> {
        let mut days: Vec<DayTotal> = (from..=to)
            .map(|day| DayTotal {
                day,
                active_users: 0,
                new_users: 0,
                messages: 0,
                bytes: 0,
            })
            .collect();
        if days.is_empty() {
            return Ok(days);
        }
        let (usage, signups) = match self {
            Store::Memory(m) => m.daily_totals(from, to),
            Store::Maria(m) => m.daily_totals(from, to).await?,
        };
        for (day, active, messages, bytes) in usage {
            let d = &mut days[(day - from) as usize];
            d.active_users = active;
            d.messages = messages;
            d.bytes = bytes;
        }
        for (day, n) in signups {
            days[(day - from) as usize].new_users = n;
        }
        Ok(days)
    }

    /// 운영자 사용자 목록: 이메일·이름에 `query`가 든 계정(빈 문자열이면 전부), 최근 가입 순.
    /// 돌려주는 첫 값은 조건에 맞는 전체 수.
    pub async fn list_users(
        &self,
        query: &str,
        offset: i64,
        today: i32,
    ) -> Result<(i64, Vec<UserSummary>)> {
        let query = query.trim().to_lowercase();
        let offset = offset.max(0);
        match self {
            Store::Memory(m) => Ok(m.list_users(&query, offset, today)),
            Store::Maria(m) => m.list_users(&query, offset, today).await,
        }
    }

    /// 테스트용: 계정의 갱신 토큰 행에 저장된 값 (해시만 있어야 한다).
    #[doc(hidden)]
    pub async fn stored_refresh_values(&self, user_id: &str) -> Result<Vec<Vec<u8>>> {
        match self {
            Store::Memory(m) => Ok(m
                .lock()
                .refresh
                .iter()
                .filter(|(_, r)| r.user_id == user_id)
                .map(|(h, _)| h.to_vec())
                .collect()),
            Store::Maria(m) => {
                let rows = sqlx::query("SELECT token_hash FROM refresh_tokens WHERE user_id = ?")
                    .bind(user_id)
                    .fetch_all(&m.pool)
                    .await?;
                rows.iter()
                    .map(|r| {
                        r.try_get::<Vec<u8>, _>("token_hash")
                            .map_err(StoreError::from)
                    })
                    .collect()
            }
        }
    }
}

/// 글자 수(문자 단위)로 자른다. 컬럼 길이를 넘지 않게.
fn clip(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

// ---------------------------------------------------------------- 메모리

#[derive(Default)]
pub struct MemStore {
    inner: Mutex<MemInner>,
}

#[derive(Default)]
struct MemInner {
    users: HashMap<String, User>,
    by_sub: HashMap<(String, String), String>,
    /// (계정, 기기) → 기기. 등록 순서를 지키려고 Vec
    devices: Vec<(String, Device)>,
    refresh: HashMap<[u8; 32], MemRefresh>,
    /// (계정, 일) → (게시 수, 바이트)
    usage: HashMap<(String, i32), (i64, i64)>,
}

struct MemRefresh {
    user_id: String,
    family: String,
    expires_at: i64,
    used_at: Option<i64>,
    revoked: bool,
}

impl MemStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, MemInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn upsert_user(&self, who: &Identity, now: i64) -> User {
        let mut g = self.lock();
        let key = (who.provider.clone(), who.sub.clone());
        if let Some(id) = g.by_sub.get(&key).cloned() {
            let u = g.users.get_mut(&id).expect("색인과 표가 맞음");
            u.email = who.email.clone();
            u.name = who.name.clone();
            u.picture_url = who.picture_url.clone();
            u.last_login_ms = now;
            return u.clone();
        }
        let u = User {
            id: crate::random_id(),
            provider: who.provider.clone(),
            provider_sub: who.sub.clone(),
            email: who.email.clone(),
            name: who.name.clone(),
            picture_url: who.picture_url.clone(),
            created_at_ms: now,
            last_login_ms: now,
        };
        g.by_sub.insert(key, u.id.clone());
        g.users.insert(u.id.clone(), u.clone());
        u
    }

    fn devices(&self, user_id: &str) -> Vec<Device> {
        self.lock()
            .devices
            .iter()
            .filter(|(u, _)| u == user_id)
            .map(|(_, d)| d.clone())
            .collect()
    }

    fn upsert_device(
        &self,
        user_id: &str,
        device_id: &str,
        name: &str,
        platform: &str,
        now: i64,
    ) -> Result<Device> {
        let mut g = self.lock();
        if !g.users.contains_key(user_id) {
            return Err(StoreError::NoUser);
        }
        if let Some((_, d)) = g
            .devices
            .iter_mut()
            .find(|(u, d)| u == user_id && d.device_id == device_id)
        {
            d.name = name.into();
            d.platform = platform.into();
            d.last_seen_ms = now;
            return Ok(d.clone());
        }
        if g.devices.iter().filter(|(u, _)| u == user_id).count() >= MAX_DEVICES {
            return Err(StoreError::TooManyDevices);
        }
        let d = Device {
            device_id: device_id.into(),
            name: name.into(),
            platform: platform.into(),
            created_at_ms: now,
            last_seen_ms: now,
        };
        g.devices.push((user_id.into(), d.clone()));
        Ok(d)
    }

    fn overview(&self, today: i32, recent: i64) -> Overview {
        let g = self.lock();
        let active = |from: i32| {
            g.usage
                .iter()
                .filter(|((_, day), (messages, _))| *day > from && *day <= today && *messages > 0)
                .map(|((u, _), _)| u)
                .collect::<std::collections::HashSet<_>>()
                .len() as i64
        };
        let mut platforms: std::collections::BTreeMap<String, PlatformCount> = Default::default();
        for (_, d) in &g.devices {
            let p = platforms
                .entry(d.platform.clone())
                .or_insert_with(|| PlatformCount {
                    platform: d.platform.clone(),
                    devices: 0,
                    seen_30d: 0,
                });
            p.devices += 1;
            if d.last_seen_ms >= recent {
                p.seen_30d += 1;
            }
        }
        Overview {
            users: g.users.len() as i64,
            dau: active(today - 1),
            mau: active(today - 30),
            platforms: platforms.into_values().collect(),
        }
    }

    #[allow(clippy::type_complexity)]
    fn daily_totals(&self, from: i32, to: i32) -> (Vec<(i32, i64, i64, i64)>, Vec<(i32, i64)>) {
        let g = self.lock();
        let mut usage: std::collections::BTreeMap<i32, (i64, i64, i64)> = Default::default();
        for ((_, day), (messages, bytes)) in &g.usage {
            if (from..=to).contains(day) {
                let e = usage.entry(*day).or_default();
                if *messages > 0 {
                    e.0 += 1;
                }
                e.1 += messages;
                e.2 += bytes;
            }
        }
        let mut signups: std::collections::BTreeMap<i32, i64> = Default::default();
        for u in g.users.values() {
            let day = u.created_at_ms.div_euclid(DAY_MS) as i32;
            if (from..=to).contains(&day) {
                *signups.entry(day).or_default() += 1;
            }
        }
        (
            usage
                .into_iter()
                .map(|(d, (a, m, b))| (d, a, m, b))
                .collect(),
            signups.into_iter().collect(),
        )
    }

    fn list_users(&self, query: &str, offset: i64, today: i32) -> (i64, Vec<UserSummary>) {
        let g = self.lock();
        let mut users: Vec<&User> = g
            .users
            .values()
            .filter(|u| {
                query.is_empty()
                    || u.email.to_lowercase().contains(query)
                    || u.name.to_lowercase().contains(query)
            })
            .collect();
        users.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        let total = users.len() as i64;
        let rows = users
            .into_iter()
            .skip(offset as usize)
            .take(ADMIN_PAGE as usize)
            .map(|u| {
                let mut s = UserSummary {
                    user: u.clone(),
                    device_count: g.devices.iter().filter(|(id, _)| *id == u.id).count() as i64,
                    messages_30d: 0,
                    bytes_30d: 0,
                    last_active_day: None,
                };
                for ((id, day), (messages, bytes)) in &g.usage {
                    if *id != u.id {
                        continue;
                    }
                    if *day > today - 30 && *day <= today {
                        s.messages_30d += messages;
                        s.bytes_30d += bytes;
                    }
                    if *messages > 0 {
                        s.last_active_day = s.last_active_day.max(Some(*day));
                    }
                }
                s
            })
            .collect();
        (total, rows)
    }

    fn rotate_refresh(&self, old: &[u8; 32], new: &[u8; 32], now: i64, new_expires: i64) -> Rotate {
        let mut g = self.lock();
        let Some(r) = g.refresh.get_mut(old) else {
            return Rotate::Invalid;
        };
        if r.revoked {
            return Rotate::Invalid;
        }
        if r.used_at.is_some() {
            let user = r.user_id.clone();
            for r in g.refresh.values_mut().filter(|r| r.user_id == user) {
                r.revoked = true;
            }
            return Rotate::Reused;
        }
        if r.expires_at < now {
            return Rotate::Invalid;
        }
        r.used_at = Some(now);
        let (user_id, family) = (r.user_id.clone(), r.family.clone());
        g.refresh.insert(
            *new,
            MemRefresh {
                user_id: user_id.clone(),
                family,
                expires_at: new_expires,
                used_at: None,
                revoked: false,
            },
        );
        Rotate::Rotated { user_id }
    }
}

// ---------------------------------------------------------------- MariaDB

pub struct MariaStore {
    pool: MySqlPool,
}

const USER_COLUMNS: &str =
    "id, provider, provider_sub, email, name, picture_url, created_at_ms, last_login_ms";

fn user_from_row(r: &sqlx::mysql::MySqlRow) -> Result<User> {
    Ok(User {
        id: r.try_get("id")?,
        provider: r.try_get("provider")?,
        provider_sub: r.try_get("provider_sub")?,
        email: r.try_get("email")?,
        name: r.try_get("name")?,
        picture_url: r.try_get("picture_url")?,
        created_at_ms: r.try_get("created_at_ms")?,
        last_login_ms: r.try_get("last_login_ms")?,
    })
}

fn device_from_row(r: &sqlx::mysql::MySqlRow) -> Result<Device> {
    Ok(Device {
        device_id: r.try_get("device_id")?,
        name: r.try_get("name")?,
        platform: r.try_get("platform")?,
        created_at_ms: r.try_get("created_at_ms")?,
        last_seen_ms: r.try_get("last_seen_ms")?,
    })
}

impl MariaStore {
    async fn upsert_user(&self, who: &Identity, now: i64) -> Result<User> {
        sqlx::query(
            "INSERT INTO users (id, provider, provider_sub, email, name, picture_url, created_at_ms, last_login_ms) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON DUPLICATE KEY UPDATE email = VALUES(email), name = VALUES(name), \
             picture_url = VALUES(picture_url), last_login_ms = VALUES(last_login_ms)",
        )
        .bind(crate::random_id())
        .bind(&who.provider)
        .bind(&who.sub)
        .bind(&who.email)
        .bind(&who.name)
        .bind(&who.picture_url)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        let row = sqlx::query(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE provider = ? AND provider_sub = ?"
        ))
        .bind(&who.provider)
        .bind(&who.sub)
        .fetch_one(&self.pool)
        .await?;
        user_from_row(&row)
    }

    async fn user(&self, id: &str) -> Result<Option<User>> {
        let row = sqlx::query(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(user_from_row).transpose()
    }

    async fn devices(&self, user_id: &str) -> Result<Vec<Device>> {
        let rows = sqlx::query(
            "SELECT device_id, name, platform, created_at_ms, last_seen_ms FROM devices \
             WHERE user_id = ? ORDER BY created_at_ms, device_id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(device_from_row).collect()
    }

    async fn upsert_device(
        &self,
        user_id: &str,
        device_id: &str,
        name: &str,
        platform: &str,
        now: i64,
    ) -> Result<Device> {
        let mut tx = self.pool.begin().await?;
        let user = sqlx::query("SELECT id FROM users WHERE id = ? FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
        if user.is_none() {
            return Err(StoreError::NoUser);
        }
        let updated = sqlx::query(
            "UPDATE devices SET name = ?, platform = ?, last_seen_ms = ? \
             WHERE user_id = ? AND device_id = ?",
        )
        .bind(name)
        .bind(platform)
        .bind(now)
        .bind(user_id)
        .bind(device_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            let count: i64 = sqlx::query("SELECT COUNT(*) AS n FROM devices WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&mut *tx)
                .await?
                .try_get("n")?;
            if count as usize >= MAX_DEVICES {
                return Err(StoreError::TooManyDevices);
            }
            sqlx::query(
                "INSERT INTO devices (user_id, device_id, name, platform, created_at_ms, last_seen_ms) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(device_id)
            .bind(name)
            .bind(platform)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        let row = sqlx::query(
            "SELECT device_id, name, platform, created_at_ms, last_seen_ms FROM devices \
             WHERE user_id = ? AND device_id = ?",
        )
        .bind(user_id)
        .bind(device_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        device_from_row(&row)
    }

    async fn insert_refresh(
        &self,
        hash: &[u8; 32],
        user_id: &str,
        family: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO refresh_tokens (token_hash, user_id, family, created_at_ms, expires_at_ms) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(hash.as_slice())
        .bind(user_id)
        .bind(family)
        .bind(now)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn rotate_refresh(
        &self,
        old: &[u8; 32],
        new: &[u8; 32],
        now: i64,
        new_expires: i64,
    ) -> Result<Rotate> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT user_id, family, expires_at_ms, used_at_ms, revoked FROM refresh_tokens \
             WHERE token_hash = ? FOR UPDATE",
        )
        .bind(old.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(Rotate::Invalid);
        };
        let user_id: String = row.try_get("user_id")?;
        let family: String = row.try_get("family")?;
        let expires_at: i64 = row.try_get("expires_at_ms")?;
        let used_at: Option<i64> = row.try_get("used_at_ms")?;
        let revoked: bool = row.try_get("revoked")?;
        if revoked {
            return Ok(Rotate::Invalid);
        }
        if used_at.is_some() {
            sqlx::query("UPDATE refresh_tokens SET revoked = 1 WHERE user_id = ?")
                .bind(&user_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(Rotate::Reused);
        }
        if expires_at < now {
            return Ok(Rotate::Invalid);
        }
        sqlx::query("UPDATE refresh_tokens SET used_at_ms = ? WHERE token_hash = ?")
            .bind(now)
            .bind(old.as_slice())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO refresh_tokens (token_hash, user_id, family, created_at_ms, expires_at_ms) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(new.as_slice())
        .bind(&user_id)
        .bind(&family)
        .bind(now)
        .bind(new_expires)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Rotate::Rotated { user_id })
    }

    async fn overview(&self, today: i32, recent: i64) -> Result<Overview> {
        let users: i64 = sqlx::query("SELECT COUNT(*) AS n FROM users")
            .fetch_one(&self.pool)
            .await?
            .try_get("n")?;
        let active = |from: i32| {
            sqlx::query(
                "SELECT COUNT(DISTINCT user_id) AS n FROM usage_daily \
                 WHERE day > ? AND day <= ? AND messages > 0",
            )
            .bind(from)
            .bind(today)
        };
        let dau: i64 = active(today - 1)
            .fetch_one(&self.pool)
            .await?
            .try_get("n")?;
        let mau: i64 = active(today - 30)
            .fetch_one(&self.pool)
            .await?
            .try_get("n")?;
        // SUM은 DECIMAL로 오므로 정수로 바꾼다
        let rows = sqlx::query(
            "SELECT platform, COUNT(*) AS devices, \
             CAST(COALESCE(SUM(last_seen_ms >= ?), 0) AS SIGNED) AS seen \
             FROM devices GROUP BY platform ORDER BY platform",
        )
        .bind(recent)
        .fetch_all(&self.pool)
        .await?;
        let platforms = rows
            .iter()
            .map(|r| {
                Ok(PlatformCount {
                    platform: r.try_get("platform")?,
                    devices: r.try_get("devices")?,
                    seen_30d: r.try_get("seen")?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Overview {
            users,
            dau,
            mau,
            platforms,
        })
    }

    #[allow(clippy::type_complexity)]
    async fn daily_totals(
        &self,
        from: i32,
        to: i32,
    ) -> Result<(Vec<(i32, i64, i64, i64)>, Vec<(i32, i64)>)> {
        let rows = sqlx::query(
            "SELECT day, COUNT(DISTINCT CASE WHEN messages > 0 THEN user_id END) AS active, \
             CAST(SUM(messages) AS SIGNED) AS messages, CAST(SUM(bytes) AS SIGNED) AS bytes \
             FROM usage_daily WHERE day >= ? AND day <= ? GROUP BY day",
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await?;
        let usage = rows
            .iter()
            .map(|r| {
                Ok((
                    r.try_get("day")?,
                    r.try_get("active")?,
                    r.try_get("messages")?,
                    r.try_get("bytes")?,
                ))
            })
            .collect::<Result<_>>()?;
        let rows = sqlx::query(
            "SELECT CAST(created_at_ms DIV ? AS SIGNED) AS day, COUNT(*) AS n FROM users \
             WHERE created_at_ms >= ? AND created_at_ms < ? GROUP BY day",
        )
        .bind(DAY_MS)
        .bind(from as i64 * DAY_MS)
        .bind((to as i64 + 1) * DAY_MS)
        .fetch_all(&self.pool)
        .await?;
        let signups = rows
            .iter()
            .map(|r| Ok((r.try_get::<i64, _>("day")? as i32, r.try_get("n")?)))
            .collect::<Result<_>>()?;
        Ok((usage, signups))
    }

    async fn list_users(
        &self,
        query: &str,
        offset: i64,
        today: i32,
    ) -> Result<(i64, Vec<UserSummary>)> {
        // LIKE의 특수 문자를 글자 그대로 찾게 한다 (기본 이스케이프 문자는 \)
        let pattern = format!(
            "%{}%",
            query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let total: i64 = sqlx::query(
            "SELECT COUNT(*) AS n FROM users WHERE ? = '' OR email LIKE ? OR name LIKE ?",
        )
        .bind(query)
        .bind(&pattern)
        .bind(&pattern)
        .fetch_one(&self.pool)
        .await?
        .try_get("n")?;
        let rows = sqlx::query(&format!(
            "SELECT {USER_COLUMNS}, \
             (SELECT COUNT(*) FROM devices d WHERE d.user_id = users.id) AS device_count, \
             (SELECT CAST(COALESCE(SUM(x.messages), 0) AS SIGNED) FROM usage_daily x \
              WHERE x.user_id = users.id AND x.day > ? AND x.day <= ?) AS messages_30d, \
             (SELECT CAST(COALESCE(SUM(x.bytes), 0) AS SIGNED) FROM usage_daily x \
              WHERE x.user_id = users.id AND x.day > ? AND x.day <= ?) AS bytes_30d, \
             (SELECT MAX(x.day) FROM usage_daily x \
              WHERE x.user_id = users.id AND x.messages > 0) AS last_active_day \
             FROM users WHERE ? = '' OR email LIKE ? OR name LIKE ? \
             ORDER BY created_at_ms DESC, id LIMIT ? OFFSET ?"
        ))
        .bind(today - 30)
        .bind(today)
        .bind(today - 30)
        .bind(today)
        .bind(query)
        .bind(&pattern)
        .bind(&pattern)
        .bind(ADMIN_PAGE)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let users = rows
            .iter()
            .map(|r| {
                Ok(UserSummary {
                    user: user_from_row(r)?,
                    device_count: r.try_get("device_count")?,
                    messages_30d: r.try_get("messages_30d")?,
                    bytes_30d: r.try_get("bytes_30d")?,
                    last_active_day: r.try_get("last_active_day")?,
                })
            })
            .collect::<Result<_>>()?;
        Ok((total, users))
    }

    async fn revoke_family_of(&self, hash: &[u8; 32]) -> Result<()> {
        let family: Option<String> =
            sqlx::query("SELECT family FROM refresh_tokens WHERE token_hash = ?")
                .bind(hash.as_slice())
                .fetch_optional(&self.pool)
                .await?
                .map(|r| r.try_get("family"))
                .transpose()?;
        if let Some(family) = family {
            sqlx::query("UPDATE refresh_tokens SET revoked = 1 WHERE family = ?")
                .bind(family)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
}
