//! 계정 서버 통합 테스트 (실제 HTTP).
//!
//! 저장소는 메모리로 항상 돌리고, `CF_TEST_MARIADB_URL`이 있으면 MariaDB로도 같은 시험을 돌린다.

use std::collections::HashMap;
use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use clip_ferry_account::api::{router, AppState};
use clip_ferry_account::google::{
    CodeExchange, GoogleError, GoogleVerifier, KeySource, RemoteKeys,
};
use clip_ferry_account::store::Store;
use clip_ferry_account::tokens::{self, AccessClaims, Signer};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const KEY_A: &str = include_str!("fixtures/test-google-a.pem");
const KEY_B: &str = include_str!("fixtures/test-google-b.pem");
const JWKS_A: &str = include_str!("fixtures/test-google-a.jwks.json");
const KID_A: &str = "test-kid-a";
const CLIENT: &str = "test-client.apps.googleusercontent.com";
const USAGE_SECRET: &str = "test-usage-secret";

// ---------------------------------------------------------------- 도우미

async fn stores() -> Vec<(&'static str, Store)> {
    // 테스트들이 동시에 마이그레이션을 돌리지 않게 연결은 하나씩 (빈 DB에서 처음 만들 때 부딪힌다)
    static CONNECT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    // 실패한 테스트의 출력에 서버 로그(`store: …` 같은 DB 오류 원문)가 함께 나오게 한다
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let mut v = vec![("memory", Store::memory())];
    if let Ok(url) = std::env::var("CF_TEST_MARIADB_URL") {
        let _one = CONNECT.lock().await;
        let s = Store::connect_mariadb(&url)
            .await
            .expect("CF_TEST_MARIADB_URL에 연결하지 못했습니다");
        v.push(("mariadb", s));
    }
    v
}

fn static_google_keys() -> KeySource {
    let jwks: Value = serde_json::from_str(JWKS_A).unwrap();
    let k = &jwks["keys"][0];
    let dk = DecodingKey::from_rsa_components(k["n"].as_str().unwrap(), k["e"].as_str().unwrap())
        .unwrap();
    KeySource::Static(HashMap::from([(KID_A.to_string(), dk)]))
}

struct Server {
    base: String,
    http: reqwest::Client,
    state: Arc<AppState>,
}

async fn start(store: Store, dev_login: bool) -> Server {
    serve(AppState {
        store,
        signer: Signer::from_seed(&[42u8; 32]),
        google: GoogleVerifier::new(vec![CLIENT.into()], static_google_keys()),
        google_desktop: None,
        google_android: None,
        dev_login,
        dev_pages: true,
        usage_secret: Some(USAGE_SECRET.into()),
        admin_emails: vec![],
        relay_url: None,
    })
    .await
}

async fn serve(state: AppState) -> Server {
    let state = Arc::new(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
        state,
    }
}

impl Server {
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn get_auth(&self, path: &str, token: Option<&str>) -> (u16, Value) {
        let mut req = self.http.get(format!("{}{path}", self.base));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn put_device(&self, token: &str, id: &str, name: &str, platform: &str) -> (u16, Value) {
        let r = self
            .http
            .put(format!("{}/v1/devices/{id}", self.base))
            .bearer_auth(token)
            .json(&json!({ "name": name, "platform": platform }))
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn google_login(&self, token: String) -> (u16, Value) {
        self.post("/v1/auth/google", json!({ "id_token": token }))
            .await
    }
}

fn now() -> i64 {
    clip_ferry_account::now_ms() / 1000
}

/// 테스트마다 다른 구글 `sub` (MariaDB에는 앞선 실행의 행이 남아 있을 수 있다)
fn new_sub() -> String {
    format!("sub-{}", clip_ferry_account::random_id())
}

fn google_claims(sub: &str) -> Value {
    json!({
        "iss": "https://accounts.google.com",
        "aud": CLIENT,
        "sub": sub,
        "email": format!("{sub}@example.com"),
        "email_verified": true,
        "name": "테스트 사용자",
        "picture": "https://example.com/p.png",
        "iat": now(),
        "exp": now() + 3600,
    })
}

fn id_token(pem: &str, kid: &str, claims: &Value) -> String {
    let mut h = Header::new(Algorithm::RS256);
    h.kid = Some(kid.into());
    jsonwebtoken::encode(
        &h,
        claims,
        &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn good_token(sub: &str) -> String {
    id_token(KEY_A, KID_A, &google_claims(sub))
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------- 시험

#[tokio::test]
async fn google_login_me_and_devices() {
    for (name, store) in stores().await {
        let srv = start(store, false).await;
        let sub = new_sub();
        let (st, body) = srv.google_login(good_token(&sub)).await;
        assert_eq!(st, 200, "{name}: {body}");
        assert_eq!(s(&body, "token_type"), "Bearer");
        assert_eq!(body["expires_in"], tokens::ACCESS_TTL_SECS);
        assert_eq!(body["user"]["email"], format!("{sub}@example.com"));
        assert_eq!(body["user"]["provider"], "google");
        let user_id = body["user"]["id"].as_str().unwrap().to_string();
        assert_eq!(user_id.len(), 32);
        let access = s(&body, "access_token");

        let (st, me) = srv.get_auth("/v1/me", Some(&access)).await;
        assert_eq!(st, 200, "{name}");
        assert_eq!(me["user"]["id"], user_id);
        assert_eq!(me["devices"].as_array().unwrap().len(), 0);

        let dev = "00112233445566778899aabbccddeeff";
        let (st, d) = srv.put_device(&access, dev, "내 PC", "windows").await;
        assert_eq!(st, 200, "{name}: {d}");
        assert_eq!(d["name"], "내 PC");
        let (st, d) = srv.put_device(&access, dev, "새 이름", "windows").await;
        assert_eq!(st, 200, "{name}");
        assert_eq!(d["name"], "새 이름");
        let (_, me) = srv.get_auth("/v1/me", Some(&access)).await;
        let devices = me["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1, "{name}");
        assert_eq!(devices[0]["platform"], "windows");

        // 잘못된 기기 요청
        assert_eq!(srv.put_device(&access, "XYZ", "a", "windows").await.0, 400);
        assert_eq!(
            srv.put_device(&access, "00112233445566778899AABBCCDDEEFF", "a", "windows")
                .await
                .0,
            400
        );
        assert_eq!(srv.put_device(&access, dev, "  ", "windows").await.0, 400);
        assert_eq!(srv.put_device(&access, dev, "a", "toaster").await.0, 400);
        assert_eq!(
            srv.put_device(&access, dev, &"가".repeat(65), "windows")
                .await
                .0,
            400
        );

        // 같은 구글 계정으로 다시 로그인 → 같은 계정, 정보 갱신
        let mut c = google_claims(&sub);
        c["name"] = json!("바뀐 이름");
        let (st, again) = srv.google_login(id_token(KEY_A, KID_A, &c)).await;
        assert_eq!(st, 200, "{name}");
        assert_eq!(again["user"]["id"], user_id);
        assert_eq!(again["user"]["name"], "바뀐 이름");
    }
}

#[tokio::test]
async fn rejects_bad_google_tokens() {
    let srv = start(Store::memory(), false).await;
    let sub = new_sub();
    let base = google_claims(&sub);
    let with = |k: &str, v: Value| {
        let mut c = base.clone();
        c[k] = v;
        c
    };
    let without = |k: &str| {
        let mut c = base.clone();
        c.as_object_mut().unwrap().remove(k);
        c
    };

    let cases: Vec<(&str, String, u16)> = vec![
        ("다른 키로 서명", id_token(KEY_B, KID_A, &base), 401),
        ("모르는 kid", id_token(KEY_A, "other-kid", &base), 401),
        (
            "만료",
            id_token(KEY_A, KID_A, &with("exp", json!(now() - 600))),
            401,
        ),
        (
            "다른 aud",
            id_token(KEY_A, KID_A, &with("aud", json!("someone-else"))),
            401,
        ),
        (
            "다른 iss",
            id_token(KEY_A, KID_A, &with("iss", json!("https://evil.example"))),
            401,
        ),
        (
            "이메일 미확인",
            id_token(KEY_A, KID_A, &with("email_verified", json!(false))),
            403,
        ),
        (
            "이메일 없음",
            id_token(KEY_A, KID_A, &without("email")),
            401,
        ),
        ("exp 없음", id_token(KEY_A, KID_A, &without("exp")), 401),
        ("JWT 아님", "not-a-jwt".to_string(), 401),
    ];
    for (what, token, want) in cases {
        let (st, body) = srv.google_login(token).await;
        assert_eq!(st, want, "{what}: {body}");
    }

    // HS256 (대칭키)로 위조: 알고리즘에서 거부
    let mut h = Header::new(Algorithm::HS256);
    h.kid = Some(KID_A.into());
    let hs = jsonwebtoken::encode(&h, &base, &EncodingKey::from_secret(b"guess")).unwrap();
    assert_eq!(srv.google_login(hs).await.0, 401);

    // 서명 부분을 바꾼 토큰
    let good = good_token(&sub);
    let mut parts: Vec<&str> = good.split('.').collect();
    let forged_body =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&with("sub", json!("victim"))).unwrap());
    parts[1] = &forged_body;
    assert_eq!(srv.google_login(parts.join(".")).await.0, 401);

    // 이 모든 시도 뒤에도 정상 토큰은 된다
    assert_eq!(srv.google_login(good).await.0, 200);
}

#[tokio::test]
async fn refresh_rotation_and_reuse_revokes_account() {
    for (name, store) in stores().await {
        let srv = start(store, false).await;
        let sub = new_sub();
        let (_, a) = srv.google_login(good_token(&sub)).await;
        let (_, b) = srv.google_login(good_token(&sub)).await; // 다른 기기의 로그인
        let a1 = s(&a, "refresh_token");
        let b1 = s(&b, "refresh_token");

        let (st, r) = srv
            .post("/v1/auth/refresh", json!({ "refresh_token": a1 }))
            .await;
        assert_eq!(st, 200, "{name}: {r}");
        let a2 = s(&r, "refresh_token");
        assert_ne!(a1, a2);
        assert_eq!(
            srv.get_auth("/v1/me", Some(&s(&r, "access_token"))).await.0,
            200
        );

        // 이미 교체에 쓴 a1을 다시 → 재사용, 그 계정의 모든 갱신 토큰 무효화
        let (st, r) = srv
            .post("/v1/auth/refresh", json!({ "refresh_token": a1 }))
            .await;
        assert_eq!(st, 401, "{name}");
        assert_eq!(r["error"], "refresh_reused");
        for t in [&a2, &b1] {
            let (st, r) = srv
                .post("/v1/auth/refresh", json!({ "refresh_token": t }))
                .await;
            assert_eq!(st, 401, "{name}: {r}");
        }

        // 다시 로그인하면 된다
        let (st, c) = srv.google_login(good_token(&sub)).await;
        assert_eq!(st, 200, "{name}");
        let (st, _) = srv
            .post(
                "/v1/auth/refresh",
                json!({ "refresh_token": s(&c, "refresh_token") }),
            )
            .await;
        assert_eq!(st, 200, "{name}");

        // 모르는 토큰
        let (st, r) = srv
            .post("/v1/auth/refresh", json!({ "refresh_token": "nope" }))
            .await;
        assert_eq!(
            (st, r["error"].as_str()),
            (401, Some("invalid_refresh")),
            "{name}"
        );
    }
}

#[tokio::test]
async fn logout_revokes_only_that_login() {
    for (name, store) in stores().await {
        let srv = start(store, false).await;
        let sub = new_sub();
        let (_, a) = srv.google_login(good_token(&sub)).await;
        let (_, b) = srv.google_login(good_token(&sub)).await;
        // a를 한 번 교체한 뒤 최신 토큰으로 로그아웃 → 묶음 전체(옛 것 포함)가 무효
        let (_, a2) = srv
            .post(
                "/v1/auth/refresh",
                json!({ "refresh_token": s(&a, "refresh_token") }),
            )
            .await;
        let (st, _) = srv
            .post(
                "/v1/auth/logout",
                json!({ "refresh_token": s(&a2, "refresh_token") }),
            )
            .await;
        assert_eq!(st, 204, "{name}");
        let (st, r) = srv
            .post(
                "/v1/auth/refresh",
                json!({ "refresh_token": s(&a2, "refresh_token") }),
            )
            .await;
        assert_eq!(
            (st, r["error"].as_str()),
            (401, Some("invalid_refresh")),
            "{name}"
        );
        // 다른 로그인(b)은 그대로
        let (st, _) = srv
            .post(
                "/v1/auth/refresh",
                json!({ "refresh_token": s(&b, "refresh_token") }),
            )
            .await;
        assert_eq!(st, 200, "{name}");
        // 없는 토큰 로그아웃도 204
        assert_eq!(
            srv.post("/v1/auth/logout", json!({ "refresh_token": "x" }))
                .await
                .0,
            204
        );
    }
}

#[tokio::test]
async fn me_requires_valid_access_token() {
    let srv = start(Store::memory(), false).await;
    assert_eq!(srv.get_auth("/v1/me", None).await.0, 401);
    assert_eq!(srv.get_auth("/v1/me", Some("garbage")).await.0, 401);
    // 다른 서명 키로 만든 토큰
    let (_, body) = srv.google_login(good_token(&new_sub())).await;
    let user_id = body["user"]["id"].as_str().unwrap();
    let (other, _) = Signer::from_seed(&[1u8; 32]).issue(user_id, now());
    assert_eq!(srv.get_auth("/v1/me", Some(&other)).await.0, 401);
    // 만료된 토큰
    let (old, _) = srv
        .state
        .signer
        .issue(user_id, now() - 2 * tokens::ACCESS_TTL_SECS);
    assert_eq!(srv.get_auth("/v1/me", Some(&old)).await.0, 401);
    // 없는 계정의 (정상 서명) 토큰
    let (ghost, _) = srv
        .state
        .signer
        .issue("ffffffffffffffffffffffffffffffff", now());
    assert_eq!(srv.get_auth("/v1/me", Some(&ghost)).await.0, 401);
}

#[tokio::test]
async fn database_keeps_only_refresh_hashes() {
    for (name, store) in stores().await {
        let srv = start(store, false).await;
        let (_, body) = srv.google_login(good_token(&new_sub())).await;
        let user_id = s(&body["user"], "id");
        let token = s(&body, "refresh_token");
        let (_, r) = srv
            .post("/v1/auth/refresh", json!({ "refresh_token": token }))
            .await;
        let token2 = s(&r, "refresh_token");

        let stored = srv
            .state
            .store
            .stored_refresh_values(&user_id)
            .await
            .unwrap();
        assert_eq!(stored.len(), 2, "{name}");
        for t in [&token, &token2] {
            assert!(
                !stored.iter().any(|v| v == t.as_bytes()),
                "{name}: 원문이 저장됨"
            );
            let h = Sha256::digest(t.as_bytes()).to_vec();
            assert!(stored.contains(&h), "{name}: 해시가 없음");
        }
    }
}

#[tokio::test]
async fn dev_login_only_when_enabled() {
    let off = start(Store::memory(), false).await;
    let (st, _) = off.post("/v1/auth/dev", json!({ "email": "a@b.c" })).await;
    assert_eq!(st, 404);

    let on = start(Store::memory(), true).await;
    let (st, body) = on
        .post(
            "/v1/auth/dev",
            json!({ "email": "Dev@Example.com", "name": "개발" }),
        )
        .await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(body["user"]["provider"], "dev");
    assert_eq!(body["user"]["email"], "dev@example.com");
    assert_eq!(
        on.post("/v1/auth/dev", json!({ "email": "nope" })).await.0,
        400
    );
}

/// 릴레이(F33)가 하듯이 `GET /v1/keys`의 공개키만으로 접속 토큰을 확인한다.
#[tokio::test]
async fn keys_endpoint_verifies_access_tokens() {
    let srv = start(Store::memory(), false).await;
    let (_, body) = srv.google_login(good_token(&new_sub())).await;
    let access = s(&body, "access_token");
    let (st, jwks) = srv.get_auth("/v1/keys", None).await;
    assert_eq!(st, 200);
    let k = &jwks["keys"][0];
    assert_eq!(k["kty"], "OKP");
    assert_eq!(k["crv"], "Ed25519");
    let header = jsonwebtoken::decode_header(&access).unwrap();
    assert_eq!(header.kid.as_deref(), k["kid"].as_str());

    let dk = DecodingKey::from_ed_components(k["x"].as_str().unwrap()).unwrap();
    let mut v = Validation::new(Algorithm::EdDSA);
    v.set_audience(&[tokens::AUDIENCE]);
    v.set_issuer(&[tokens::ISSUER]);
    let claims = jsonwebtoken::decode::<AccessClaims>(&access, &dk, &v)
        .unwrap()
        .claims;
    assert_eq!(claims.sub, body["user"]["id"].as_str().unwrap());
}

/// 구글 JWKS를 HTTP로 받아 캐시하는 경로.
#[tokio::test]
async fn remote_jwks_fetch() {
    use axum::routing::get;
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let h = hits.clone();
    let app = axum::Router::new().route(
        "/certs",
        get(move || {
            h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async {
                (
                    [("cache-control", "public, max-age=600")],
                    [("content-type", "application/json")],
                    JWKS_A,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let v = GoogleVerifier::new(
        vec![CLIENT.into()],
        KeySource::Remote(RemoteKeys::new(format!("http://{addr}/certs"))),
    );
    let sub = new_sub();
    let who = v.verify(&good_token(&sub)).await.unwrap();
    assert_eq!(who.sub, sub);
    assert_eq!(who.email, format!("{sub}@example.com"));
    // 두 번째는 캐시에서
    v.verify(&good_token(&sub)).await.unwrap();
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    // 모르는 kid: 방금 받았으므로 다시 받지 않고 거부
    let err = v
        .verify(&id_token(KEY_A, "rotated", &google_claims(&sub)))
        .await;
    assert_eq!(err, Err(GoogleError::Invalid("unknown kid")));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn remote_jwks_unreachable() {
    // 아무도 듣지 않는 주소
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let v = GoogleVerifier::new(
        vec![CLIENT.into()],
        KeySource::Remote(RemoteKeys::new(format!("http://{addr}/certs"))),
    );
    assert_eq!(
        v.verify(&good_token(&new_sub())).await,
        Err(GoogleError::KeysUnavailable)
    );
}

#[tokio::test]
async fn dev_page_has_client_id() {
    let srv = start(Store::memory(), false).await;
    let html = srv
        .http
        .get(format!("{}/dev/google", srv.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains(&format!("data-client_id=\"{CLIENT}\"")));
    // 페이지가 /dev/ 아래에 있으므로 API는 절대 경로로 불러야 한다
    assert!(html.contains("'/v1/auth/google'"));
    assert!(!html.contains("'v1/"));
}

#[tokio::test]
async fn internal_usage_from_relay() {
    for (name, store) in stores().await {
        let srv = start(store, false).await;
        let (_, body) = srv.google_login(good_token(&new_sub())).await;
        let user_id = s(&body["user"], "id");
        let report = json!({ "entries": [
            { "user_id": user_id, "messages": 3, "bytes": 120 },
            { "user_id": "ffffffffffffffffffffffffffffffff", "messages": 1, "bytes": 1 },
        ]});
        let post = |secret: Option<&str>| {
            let mut req = srv
                .http
                .post(format!("{}/internal/usage", srv.base))
                .json(&report);
            if let Some(t) = secret {
                req = req.bearer_auth(t);
            }
            req.send()
        };
        assert_eq!(post(None).await.unwrap().status().as_u16(), 401, "{name}");
        assert_eq!(
            post(Some("wrong")).await.unwrap().status().as_u16(),
            401,
            "{name}"
        );
        let r = post(Some(USAGE_SECRET)).await.unwrap();
        assert_eq!(r.status().as_u16(), 200, "{name}");
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["accepted"], 1, "{name}: 없는 계정은 건너뛴다");
        post(Some(USAGE_SECRET)).await.unwrap();

        let usage = srv.state.store.usage(&user_id).await.unwrap();
        assert_eq!(usage.len(), 1, "{name}");
        let (_, me) = srv
            .get_auth("/v1/me", Some(&s(&body, "access_token")))
            .await;
        assert_eq!(me["usage"][0]["messages"], 6, "{name}: /v1/me에 보인다");
        assert_eq!(
            (usage[0].messages, usage[0].bytes),
            (6, 240),
            "{name}: 같은 날은 더한다"
        );
        assert_eq!(
            usage[0].day as i64,
            clip_ferry_account::now_ms() / 86_400_000
        );
    }
}

// ---------------------------------------------------------------- 데스크톱 인가 코드 (F34)

const DESKTOP_SECRET: &str = "test-desktop-secret";
const GOOD_CODE: &str = "good-code";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

/// 가짜 구글 토큰 주소. 맞는 코드·검증자·비밀이 오면 `id_token`을 준다. 받은 요청 수를 센다.
async fn fake_google_token(id_token: String) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use axum::routing::post;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let app = axum::Router::new().route(
        "/token",
        post(move |axum::Form(f): axum::Form<HashMap<String, String>>| {
            let id_token = id_token.clone();
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                let ok = f.get("grant_type").map(String::as_str) == Some("authorization_code")
                    && f.get("code").map(String::as_str) == Some(GOOD_CODE)
                    && f.get("code_verifier").map(String::as_str) == Some(VERIFIER)
                    && f.get("client_id").map(String::as_str) == Some(CLIENT)
                    && f.get("client_secret").map(String::as_str) == Some(DESKTOP_SECRET)
                    && f.get("redirect_uri")
                        .is_some_and(|u| u.starts_with("http://127.0.0.1:"));
                if ok {
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(json!({ "id_token": id_token })),
                    )
                } else {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(json!({ "error": "invalid_grant" })),
                    )
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/token"), hits)
}

async fn start_desktop(token_url: &str) -> Server {
    let state = Arc::new(AppState {
        store: Store::memory(),
        signer: Signer::from_seed(&[42u8; 32]),
        google: GoogleVerifier::new(vec![CLIENT.into()], static_google_keys()),
        google_desktop: Some(CodeExchange::new(token_url, CLIENT, DESKTOP_SECRET)),
        google_android: None,
        dev_login: false,
        dev_pages: false,
        usage_secret: None,
        admin_emails: vec![],
        relay_url: None,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
        state,
    }
}

fn code_body(code: &str, verifier: &str, redirect: &str) -> Value {
    json!({ "code": code, "code_verifier": verifier, "redirect_uri": redirect })
}

#[tokio::test]
async fn desktop_code_login() {
    let sub = new_sub();
    let (token_url, hits) = fake_google_token(good_token(&sub)).await;
    let srv = start_desktop(&token_url).await;
    let redirect = "http://127.0.0.1:53124/";

    let (st, cfg) = srv.get_auth("/v1/auth/google/desktop", None).await;
    assert_eq!(st, 200);
    assert_eq!(cfg["client_id"], CLIENT);

    let (st, body) = srv
        .post(
            "/v1/auth/google/code",
            code_body(GOOD_CODE, VERIFIER, redirect),
        )
        .await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(body["user"]["email"], format!("{sub}@example.com"));
    assert_eq!(body["user"]["provider"], "google");
    let (st, _) = srv
        .get_auth("/v1/me", Some(&s(&body, "access_token")))
        .await;
    assert_eq!(st, 200);
    // 구글 화면 로그인과 같은 계정이 된다
    let (_, again) = srv.google_login(good_token(&sub)).await;
    assert_eq!(again["user"]["id"], body["user"]["id"]);

    // 구글이 받지 않는 코드·검증자
    let (st, e) = srv
        .post(
            "/v1/auth/google/code",
            code_body("wrong", VERIFIER, redirect),
        )
        .await;
    assert_eq!((st, s(&e, "error").as_str()), (401, "invalid_code"));
    let other = "x".repeat(43);
    let (st, _) = srv
        .post(
            "/v1/auth/google/code",
            code_body(GOOD_CODE, &other, redirect),
        )
        .await;
    assert_eq!(st, 401);

    // 구글에 묻기 전에 막는 것: 루프백이 아닌 돌아올 주소, 짧은 검증자
    let before = hits.load(std::sync::atomic::Ordering::SeqCst);
    for bad in ["https://evil.example/cb", "http://localhost:53124/"] {
        let (st, e) = srv
            .post("/v1/auth/google/code", code_body(GOOD_CODE, VERIFIER, bad))
            .await;
        assert_eq!(
            (st, s(&e, "error").as_str()),
            (400, "bad_redirect_uri"),
            "{bad}"
        );
    }
    let (st, _) = srv
        .post(
            "/v1/auth/google/code",
            code_body(GOOD_CODE, "short", redirect),
        )
        .await;
    assert_eq!(st, 400);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), before);
}

#[tokio::test]
async fn desktop_code_login_checks_the_id_token() {
    // 구글 토큰 주소가 (가로채여) 다른 클라이언트용·위조 토큰을 주어도 받지 않는다
    let mut c = google_claims(&new_sub());
    c["aud"] = json!("someone-else.apps.googleusercontent.com");
    for token in [
        id_token(KEY_A, KID_A, &c),
        id_token(KEY_B, KID_A, &google_claims("x")),
    ] {
        let (token_url, _) = fake_google_token(token).await;
        let srv = start_desktop(&token_url).await;
        let (st, e) = srv
            .post(
                "/v1/auth/google/code",
                code_body(GOOD_CODE, VERIFIER, "http://127.0.0.1:1/"),
            )
            .await;
        assert_eq!((st, s(&e, "error").as_str()), (401, "invalid_id_token"));
    }
}

#[tokio::test]
async fn desktop_login_off_without_client() {
    let srv = start(Store::memory(), false).await;
    let (st, _) = srv.get_auth("/v1/auth/google/desktop", None).await;
    assert_eq!(st, 404);
    let (st, e) = srv
        .post(
            "/v1/auth/google/code",
            code_body(GOOD_CODE, VERIFIER, "http://127.0.0.1:1/"),
        )
        .await;
    assert_eq!(
        (st, s(&e, "error").as_str()),
        (404, "desktop_login_disabled")
    );
}

#[tokio::test]
async fn desktop_code_google_unreachable() {
    let srv = start_desktop("http://127.0.0.1:9/token").await;
    let (st, e) = srv
        .post(
            "/v1/auth/google/code",
            code_body(GOOD_CODE, VERIFIER, "http://127.0.0.1:1/"),
        )
        .await;
    assert_eq!((st, s(&e, "error").as_str()), (503, "google_unavailable"));
}

/// Android 앱은 계정 서버에서 웹 클라이언트 ID를 받아 Credential Manager에 넘기고,
/// 받은 ID 토큰(`aud` = 웹 클라이언트)을 `POST /v1/auth/google`로 낸다 (F35).
#[tokio::test]
async fn android_client_id_config() {
    let srv = start(Store::memory(), false).await;
    let (st, e) = srv.get_auth("/v1/auth/google/android", None).await;
    assert_eq!(
        (st, s(&e, "error").as_str()),
        (404, "android_login_disabled")
    );

    let srv = serve(AppState {
        store: Store::memory(),
        signer: Signer::from_seed(&[42u8; 32]),
        google: GoogleVerifier::new(vec![CLIENT.into()], static_google_keys()),
        google_desktop: None,
        google_android: Some(CLIENT.into()),
        dev_login: false,
        dev_pages: false,
        usage_secret: None,
        admin_emails: vec![],
        relay_url: None,
    })
    .await;
    let (st, v) = srv.get_auth("/v1/auth/google/android", None).await;
    assert_eq!((st, s(&v, "client_id")), (200, CLIENT.to_string()));
    // 그 클라이언트로 발급된 ID 토큰으로 로그인된다
    let (st, v) = srv.google_login(good_token(&new_sub())).await;
    assert_eq!(st, 200, "{v}");
    assert!(!s(&v, "refresh_token").is_empty());
}

/// 앱이 받아 쓸 릴레이 주소 (F43). 로그인 없이 읽고, 없으면 `null`.
#[tokio::test]
async fn app_config_tells_relay_url() {
    let srv = start(Store::memory(), false).await;
    let (st, v) = srv.get_auth("/v1/config", None).await;
    assert_eq!(st, 200);
    assert!(v["relay_url"].is_null(), "{v}");

    let srv = serve(AppState {
        store: Store::memory(),
        signer: Signer::from_seed(&[42u8; 32]),
        google: GoogleVerifier::new(vec![CLIENT.into()], static_google_keys()),
        google_desktop: None,
        google_android: None,
        dev_login: false,
        dev_pages: false,
        usage_secret: None,
        admin_emails: vec![],
        relay_url: Some("wss://relay.example.com".into()),
    })
    .await;
    let (st, v) = srv.get_auth("/v1/config", None).await;
    assert_eq!(
        (st, s(&v, "relay_url")),
        (200, "wss://relay.example.com".to_string())
    );
}

/// 계정 연결(F37): 로그인한 기기의 공개키에 서명한 참여 확인서. 같은 계정 기기가 `/v1/keys`로 확인한다.
#[tokio::test]
async fn link_ticket_is_signed_for_the_logged_in_account() {
    use ed25519_dalek::{Signature, VerifyingKey};
    for (_, store) in stores().await {
        let srv = start(store, false).await;
        let (_, body) = srv.google_login(good_token(&new_sub())).await;
        let access = s(&body, "access_token");
        let user_id = body["user"]["id"].as_str().unwrap().to_string();
        let req = json!({
            "sign_pub": URL_SAFE_NO_PAD.encode([5u8; 32]),
            "dh_pub": URL_SAFE_NO_PAD.encode([6u8; 32]),
            "name": "새 폰",
        });

        let post = |token: Option<String>, body: Value| {
            let mut r = srv
                .http
                .post(format!("{}/v1/link/ticket", srv.base))
                .json(&body);
            if let Some(t) = token {
                r = r.bearer_auth(t);
            }
            async move {
                let r = r.send().await.unwrap();
                (
                    r.status().as_u16(),
                    r.json::<Value>().await.unwrap_or(Value::Null),
                )
            }
        };
        assert_eq!(post(None, req.clone()).await.0, 401);
        assert_eq!(post(Some("nope".into()), req.clone()).await.0, 401);
        let mut bad = req.clone();
        bad["dh_pub"] = json!("short");
        assert_eq!(
            post(Some(access.clone()), bad).await,
            (400, json!({"error": "bad_key"}))
        );

        let (st, t) = post(Some(access.clone()), req).await;
        assert_eq!(st, 200, "{t}");
        assert_eq!(t["user_id"], user_id.as_str());
        assert_eq!(t["name"], "새 폰");
        let expires = t["expires_at_ms"].as_u64().unwrap();
        assert!(expires > now() as u64 * 1000);

        let (_, jwks) = srv.get_auth("/v1/keys", None).await;
        let k = &jwks["keys"][0];
        assert_eq!(t["kid"], k["kid"]);
        let public: [u8; 32] = URL_SAFE_NO_PAD
            .decode(k["x"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let sig: [u8; 64] = URL_SAFE_NO_PAD
            .decode(t["signature"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let msg = clip_ferry_account::link::ticket_message(
            &user_id, &[5u8; 32], &[6u8; 32], "새 폰", expires,
        )
        .unwrap();
        VerifyingKey::from_bytes(&public)
            .unwrap()
            .verify_strict(&msg, &Signature::from_bytes(&sig))
            .expect("공개키로 확인되어야 한다");
    }
}

// ---------------------------------------------------------------- 운영자 통계 (F38)

async fn start_admin(store: Store, admins: &[&str]) -> Server {
    serve(AppState {
        store,
        signer: Signer::from_seed(&[42u8; 32]),
        google: GoogleVerifier::new(vec![CLIENT.into()], static_google_keys()),
        google_desktop: None,
        google_android: None,
        dev_login: false,
        dev_pages: false,
        usage_secret: Some(USAGE_SECRET.into()),
        admin_emails: clip_ferry_account::admin::normalize_emails(admins),
        relay_url: None,
    })
    .await
}

impl Server {
    /// 구글로 운영자 로그인 → (상태, 응답)
    async fn admin_login(&self, sub: &str) -> (u16, Value) {
        self.post("/admin/api/session", json!({ "id_token": good_token(sub) }))
            .await
    }

    async fn report_usage(&self, user_id: &str, messages: u64, bytes: u64) {
        let r = self
            .http
            .post(format!("{}/internal/usage", self.base))
            .bearer_auth(USAGE_SECRET)
            .json(&json!({ "entries": [{ "user_id": user_id, "messages": messages, "bytes": bytes }] }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 200);
    }
}

/// 테스트마다 다른 기기 ID (MariaDB에는 앞선 실행의 행이 남는다)
fn new_device_id() -> String {
    clip_ferry_account::random_id()
}

#[tokio::test]
async fn admin_is_off_without_allow_list() {
    let srv = start(Store::memory(), false).await;
    let r = srv
        .http
        .get(format!("{}/admin", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404);
    let (st, _) = srv
        .post(
            "/admin/api/session",
            json!({ "id_token": good_token(&new_sub()) }),
        )
        .await;
    assert_eq!(st, 404);
    assert_eq!(srv.get_auth("/admin/api/stats", None).await.0, 404);
}

#[tokio::test]
async fn admin_only_for_allow_listed_google_accounts() {
    for (name, store) in stores().await {
        let op = new_sub();
        // 허용 목록은 대소문자를 가리지 않는다
        let listed = format!("{}@EXAMPLE.com", op.to_uppercase());
        let srv = start_admin(store, &[&listed]).await;

        // 허용 목록 밖 구글 계정 → 403, 토큰 없음
        let (st, body) = srv.admin_login(&new_sub()).await;
        assert_eq!(st, 403, "{name}: {body}");
        assert_eq!(body["error"], "not_admin");
        assert!(body.get("token").is_none());

        // 다른 앱용 ID 토큰 → 401
        let mut c = google_claims(&op);
        c["aud"] = json!("other-client");
        let (st, _) = srv
            .post(
                "/admin/api/session",
                json!({ "id_token": id_token(KEY_A, KID_A, &c) }),
            )
            .await;
        assert_eq!(st, 401, "{name}");

        // 이메일 미확인 → 403
        let mut c = google_claims(&op);
        c["email_verified"] = json!(false);
        let (st, _) = srv
            .post(
                "/admin/api/session",
                json!({ "id_token": id_token(KEY_A, KID_A, &c) }),
            )
            .await;
        assert_eq!(st, 403, "{name}");

        // 허용된 계정 → 운영자 토큰
        let (st, body) = srv.admin_login(&op).await;
        assert_eq!(st, 200, "{name}: {body}");
        assert_eq!(body["email"], format!("{op}@example.com"));
        assert_eq!(body["expires_in"], tokens::ADMIN_TTL_SECS);
        let admin = s(&body, "token");
        assert_eq!(
            srv.get_auth("/admin/api/stats", Some(&admin)).await.0,
            200,
            "{name}"
        );
        assert_eq!(
            srv.get_auth("/admin/api/users", Some(&admin)).await.0,
            200,
            "{name}"
        );

        // 운영자 토큰은 앱 API에 못 쓴다 (대상이 다르다)
        assert_eq!(srv.get_auth("/v1/me", Some(&admin)).await.0, 401, "{name}");

        // 토큰 없음·틀린 토큰·앱 접속 토큰 → 401
        assert_eq!(
            srv.get_auth("/admin/api/stats", None).await.0,
            401,
            "{name}"
        );
        assert_eq!(
            srv.get_auth("/admin/api/stats", Some("x.y.z")).await.0,
            401,
            "{name}"
        );
        let (_, user) = srv.google_login(good_token(&new_sub())).await;
        let access = s(&user, "access_token");
        assert_eq!(
            srv.get_auth("/admin/api/stats", Some(&access)).await.0,
            401,
            "{name}"
        );
        assert_eq!(
            srv.get_auth("/admin/api/users", Some(&access)).await.0,
            401,
            "{name}"
        );

        // 서버 키로 서명됐어도 허용 목록 밖 이메일이면 403 (목록에서 빠진 운영자)
        let (removed, _) = srv.state.signer.issue_admin("gone@example.com", now());
        let (st, body) = srv.get_auth("/admin/api/stats", Some(&removed)).await;
        assert_eq!(st, 403, "{name}: {body}");
        assert_eq!(
            srv.get_auth("/admin/api/users", Some(&removed)).await.0,
            403,
            "{name}"
        );
        let id = s(&user["user"], "id");
        assert_eq!(
            srv.get_auth(&format!("/admin/api/users/{id}"), Some(&removed))
                .await
                .0,
            403,
            "{name}"
        );
    }
}

/// 전체 합계의 증가분. 메모리 저장소는 테스트마다 따로라 정확히 맞아야 하고,
/// MariaDB는 다른 테스트가 같은 DB에 동시에 쓰므로 적어도 그만큼 늘었는지만 본다.
fn grew(store: &str, got: i64, want: i64, what: &str) {
    if store == "memory" {
        assert_eq!(got, want, "{store}: {what}");
    } else {
        assert!(got >= want, "{store}: {what} {got} < {want}");
    }
}

#[tokio::test]
async fn admin_stats_users_and_detail() {
    for (name, store) in stores().await {
        let op = new_sub();
        let srv = start_admin(store, &[&format!("{op}@example.com")]).await;
        let (_, body) = srv.admin_login(&op).await;
        let admin = s(&body, "token");
        let (_, before) = srv.get_auth("/admin/api/stats?days=7", Some(&admin)).await;

        // 사용자 둘: A는 기기 2대에 전송, B는 기기 1대에 전송 없음
        let mark = clip_ferry_account::random_id();
        let (_, a) = srv.google_login(good_token(&format!("sub-{mark}-a"))).await;
        let (_, b) = srv.google_login(good_token(&format!("sub-{mark}-b"))).await;
        let (a_id, b_id) = (s(&a["user"], "id"), s(&b["user"], "id"));
        let (a_tok, b_tok) = (s(&a, "access_token"), s(&b, "access_token"));
        srv.put_device(&a_tok, &new_device_id(), "A의 PC", "windows")
            .await;
        srv.put_device(&a_tok, &new_device_id(), "A의 폰", "android")
            .await;
        srv.put_device(&b_tok, &new_device_id(), "B의 폰", "android")
            .await;
        srv.report_usage(&a_id, 4, 1000).await;
        srv.report_usage(&a_id, 1, 24).await;

        let (st, after) = srv.get_auth("/admin/api/stats?days=7", Some(&admin)).await;
        assert_eq!(st, 200, "{name}: {after}");
        let delta = |k: &str| {
            after["overview"][k].as_i64().unwrap() - before["overview"][k].as_i64().unwrap()
        };
        grew(name, delta("users"), 2, "가입자");
        grew(name, delta("dau"), 1, "DAU: 전송한 계정만 활성");
        grew(name, delta("mau"), 1, "MAU");
        let platform = |v: &Value, p: &str, k: &str| {
            v["overview"]["platforms"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["platform"] == p)
                .map(|x| x[k].as_i64().unwrap())
                .unwrap_or(0)
        };
        grew(
            name,
            platform(&after, "android", "devices") - platform(&before, "android", "devices"),
            2,
            "android 기기",
        );
        grew(
            name,
            platform(&after, "windows", "seen_30d") - platform(&before, "windows", "seen_30d"),
            1,
            "windows 최근 접속",
        );
        let daily = after["daily"].as_array().unwrap();
        assert_eq!(daily.len(), 7, "{name}: 빈 날도 채운다");
        let today = after["today"].as_i64().unwrap();
        assert_eq!(daily[6]["day"], today, "{name}: 오래된 날부터");
        let day_delta =
            |k: &str| daily[6][k].as_i64().unwrap() - before["daily"][6][k].as_i64().unwrap();
        grew(name, day_delta("messages"), 5, "오늘 messages");
        grew(name, day_delta("bytes"), 1024, "오늘 bytes");
        grew(name, day_delta("new_users"), 2, "오늘 new_users");
        grew(name, day_delta("active_users"), 1, "오늘 active_users");

        // 목록: 검색
        let (st, list) = srv
            .get_auth(&format!("/admin/api/users?q={mark}"), Some(&admin))
            .await;
        assert_eq!(st, 200, "{name}: {list}");
        assert_eq!(list["total"], 2, "{name}");
        let users = list["users"].as_array().unwrap();
        let row_a = users.iter().find(|u| u["id"] == a_id.as_str()).unwrap();
        assert_eq!(row_a["device_count"], 2, "{name}");
        assert_eq!(row_a["messages_30d"], 5, "{name}");
        assert_eq!(row_a["bytes_30d"], 1024, "{name}");
        assert_eq!(row_a["last_active_day"], today, "{name}");
        assert!(
            row_a.get("provider_sub").is_none(),
            "{name}: 구글 sub는 내보내지 않는다"
        );
        let row_b = users.iter().find(|u| u["id"] == b_id.as_str()).unwrap();
        assert_eq!(row_b["last_active_day"], Value::Null, "{name}");
        // LIKE 특수 문자는 글자 그대로 찾는다
        let (_, none) = srv
            .get_auth("/admin/api/users?q=%25%25nobody_", Some(&admin))
            .await;
        assert_eq!(none["total"], 0, "{name}: % _ 는 와일드카드가 아니다");
        let (_, all) = srv.get_auth("/admin/api/users", Some(&admin)).await;
        assert!(all["total"].as_i64().unwrap() >= 2, "{name}");
        assert!(all["users"].as_array().unwrap().len() <= 50, "{name}");

        // 상세
        let (st, d) = srv
            .get_auth(&format!("/admin/api/users/{a_id}"), Some(&admin))
            .await;
        assert_eq!(st, 200, "{name}: {d}");
        assert_eq!(d["user"]["email"], format!("sub-{mark}-a@example.com"));
        assert_eq!(d["devices"].as_array().unwrap().len(), 2, "{name}");
        assert_eq!(d["usage"][0]["messages"], 5, "{name}");
        assert!(d["user"].get("provider_sub").is_none(), "{name}");
        let missing = format!("/admin/api/users/{}", "f".repeat(32));
        assert_eq!(srv.get_auth(&missing, Some(&admin)).await.0, 404, "{name}");
        assert_eq!(
            srv.get_auth("/admin/api/users/nothex", Some(&admin))
                .await
                .0,
            404,
            "{name}"
        );
    }
}

#[tokio::test]
async fn admin_page_is_locked_down() {
    let srv = start_admin(Store::memory(), &["op@example.com"]).await;
    let r = srv
        .http
        .get(format!("{}/admin", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let csp = r.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        csp.contains("script-src 'self' https://accounts.google.com/gsi/client;"),
        "{csp}"
    );
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(r.headers()["cache-control"], "no-store");
    let html = r.text().await.unwrap();
    assert!(html.contains(&format!("data-client_id=\"{CLIENT}\"")));
    assert!(html.contains("/admin/admin.js"));
    let js = srv
        .http
        .get(format!("{}/admin/admin.js", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(js.status().as_u16(), 200);
    assert!(js.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/javascript"));
    assert!(
        !js.text().await.unwrap().contains("innerHTML"),
        "값은 textContent로만 넣는다"
    );
}

// ---------------------------------------------------------------- 계정 삭제 (F39)

impl Server {
    async fn delete_me(&self, token: Option<&str>) -> u16 {
        let mut req = self.http.delete(format!("{}/v1/me", self.base));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.send().await.unwrap().status().as_u16()
    }
}

#[tokio::test]
async fn delete_account_removes_every_row() {
    for (name, store) in stores().await {
        let srv = start(store, false).await;
        let sub = new_sub();
        let (_, first) = srv.google_login(good_token(&sub)).await;
        let user_id = s(&first["user"], "id");
        let access = s(&first, "access_token");
        // 같은 계정의 다른 기기(다른 로그인)
        let (_, second) = srv.google_login(good_token(&sub)).await;
        assert_eq!(second["user"]["id"], user_id.as_str());
        let device = clip_ferry_account::random_id();
        assert_eq!(
            srv.put_device(&access, &device, "내 PC", "windows").await.0,
            200
        );
        let r = srv
            .http
            .post(format!("{}/internal/usage", srv.base))
            .bearer_auth(USAGE_SECRET)
            .json(&json!({ "entries": [{ "user_id": user_id, "messages": 2, "bytes": 10 }] }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 200);
        let rows = srv.state.store.rows_of(&user_id).await.unwrap();
        assert_eq!(rows, [1, 1, 2, 1], "{name}: 지우기 전에는 표마다 행이 있다");

        // 토큰 없이는 못 지운다
        assert_eq!(srv.delete_me(None).await, 401, "{name}");
        assert_eq!(srv.delete_me(Some("x.y.z")).await, 401, "{name}");

        assert_eq!(srv.delete_me(Some(&access)).await, 204, "{name}");
        assert_eq!(
            srv.state.store.rows_of(&user_id).await.unwrap(),
            [0, 0, 0, 0],
            "{name}: 어느 표에도 그 계정의 행이 남지 않는다"
        );

        // 같은 요청이 다시 와도 204
        assert_eq!(srv.delete_me(Some(&access)).await, 204, "{name}");

        // 남은 접속 토큰으로는 계정 API를 못 쓴다
        assert_eq!(srv.get_auth("/v1/me", Some(&access)).await.0, 401, "{name}");
        assert_eq!(
            srv.put_device(&access, &device, "내 PC", "windows").await.0,
            401,
            "{name}"
        );
        // 두 로그인 모두 갱신 실패 → 앱은 로그아웃된다 (다시 연결할 새 접속 토큰을 못 받는다)
        for login in [&first, &second] {
            let (st, body) = srv
                .post(
                    "/v1/auth/refresh",
                    json!({ "refresh_token": s(login, "refresh_token") }),
                )
                .await;
            assert_eq!(st, 401, "{name}: {body}");
        }
        // 릴레이가 늦게 보낸 사용량도 다시 생기지 않는다
        let r = srv
            .http
            .post(format!("{}/internal/usage", srv.base))
            .bearer_auth(USAGE_SECRET)
            .json(&json!({ "entries": [{ "user_id": user_id, "messages": 1, "bytes": 1 }] }))
            .send()
            .await
            .unwrap();
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["accepted"], 0, "{name}");
        assert_eq!(
            srv.state.store.rows_of(&user_id).await.unwrap(),
            [0, 0, 0, 0]
        );

        // 같은 구글 계정으로 다시 로그인하면 새 계정이다 (기기·사용량 없음)
        let (st, again) = srv.google_login(good_token(&sub)).await;
        assert_eq!(st, 200, "{name}");
        assert_ne!(again["user"]["id"], user_id.as_str(), "{name}");
        let (_, me) = srv
            .get_auth("/v1/me", Some(&s(&again, "access_token")))
            .await;
        assert_eq!(me["devices"].as_array().unwrap().len(), 0, "{name}");
        assert_eq!(me["usage"].as_array().unwrap().len(), 0, "{name}");
    }
}

#[tokio::test]
async fn deleting_one_account_leaves_others() {
    for (name, store) in stores().await {
        let srv = start(store, false).await;
        let (_, a) = srv.google_login(good_token(&new_sub())).await;
        let (_, b) = srv.google_login(good_token(&new_sub())).await;
        let b_id = s(&b["user"], "id");
        srv.put_device(
            &s(&b, "access_token"),
            &clip_ferry_account::random_id(),
            "B",
            "android",
        )
        .await;
        assert_eq!(srv.delete_me(Some(&s(&a, "access_token"))).await, 204);
        assert_eq!(
            srv.state.store.rows_of(&b_id).await.unwrap()[..3],
            [1, 1, 1],
            "{name}"
        );
        let (st, _) = srv
            .post(
                "/v1/auth/refresh",
                json!({ "refresh_token": s(&b, "refresh_token") }),
            )
            .await;
        assert_eq!(st, 200, "{name}");
    }
}

#[tokio::test]
async fn prune_drops_usage_older_than_retention() {
    for (name, store) in stores().await {
        let srv = start(store, false).await;
        let (_, body) = srv.google_login(good_token(&new_sub())).await;
        let user_id = s(&body["user"], "id");
        let now = clip_ferry_account::now_ms();
        let today = (now / 86_400_000) as i32;
        let keep = today - clip_ferry_account::store::USAGE_RETENTION_DAYS + 1;
        let drop = today - clip_ferry_account::store::USAGE_RETENTION_DAYS;
        for day in [drop, keep, today] {
            srv.state
                .store
                .add_usage(day, &[(user_id.clone(), 1, 1)])
                .await
                .unwrap();
        }
        let (_, usage) = srv.state.store.prune(now).await.unwrap();
        assert!(usage >= 1, "{name}");
        let days: Vec<i32> = srv
            .state
            .store
            .usage(&user_id)
            .await
            .unwrap()
            .iter()
            .map(|d| d.day)
            .collect();
        assert_eq!(days, vec![keep, today], "{name}: 13개월 지난 날만 지운다");
    }
}
