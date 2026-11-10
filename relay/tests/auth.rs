//! 접속 토큰 확인 (F33): 실제 계정 서버(메모리 저장소) + 릴레이 + 코어 전송 계층.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clip_ferry_account::api::{router, AppState};
use clip_ferry_account::google::{GoogleVerifier, KeySource};
use clip_ferry_account::store::Store;
use clip_ferry_account::tokens::Signer;
use clip_ferry_core::pb::ErrorCode;
use clip_ferry_core::transport::{Client, Event, TokenSource, TransportConfig, SUBPROTOCOL};
use clip_ferry_relay::auth::{AuthConfig, Secret};
use clip_ferry_relay::{spawn, Config, RelayHandle};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

const WAIT: Duration = Duration::from_secs(5);
const SECRET: &str = "test-usage-secret";

// ---------------------------------------------------------------- 도우미

struct Account {
    url: String,
    state: Arc<AppState>,
}

async fn account() -> Account {
    let state = Arc::new(AppState {
        store: Store::memory(),
        signer: Signer::from_seed(&[9u8; 32]),
        google: GoogleVerifier::new(vec![], KeySource::Static(HashMap::new())),
        google_desktop: None,
        google_android: None,
        dev_login: true,
        dev_pages: false,
        usage_secret: Some(SECRET.into()),
        admin_emails: vec![],
        relay_url: None,
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Account {
        url: format!("http://{addr}"),
        state,
    }
}

impl Account {
    /// 개발용 로그인 → (계정 ID, 접속 토큰)
    async fn login(&self, email: &str) -> (String, String) {
        let body: Value = reqwest::Client::new()
            .post(format!("{}/v1/auth/dev", self.url))
            .json(&json!({ "email": email }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        (
            body["user"]["id"].as_str().unwrap().to_string(),
            body["access_token"].as_str().unwrap().to_string(),
        )
    }
}

async fn relay_with(auth: Option<AuthConfig>) -> RelayHandle {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = spawn(
        listener,
        Config {
            auth,
            ..Config::default()
        },
    );
    // 공개키를 받을 때까지 (계정 서버가 떠 있으면 금방)
    for _ in 0..100 {
        if relay.auth_ready() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    relay
}

fn auth_config(account: &Account) -> AuthConfig {
    AuthConfig {
        usage_secret: Some(Secret(SECRET.into())),
        ..AuthConfig::new(&account.url)
    }
}

fn url(relay: &RelayHandle) -> String {
    format!("ws://{}", relay.local_addr())
}

fn fast(token: Option<TokenSource>) -> TransportConfig {
    TransportConfig {
        backoff_initial: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        token,
        ..TransportConfig::default()
    }
}

struct Peer {
    client: Client,
    rx: UnboundedReceiver<Event>,
}

impl Peer {
    fn connect(url: &str, token: Option<&str>) -> Peer {
        let (client, rx) = Client::connect(url, fast(token.map(TokenSource::fixed)));
        Peer { client, rx }
    }

    async fn wait<T>(&mut self, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
        timeout(WAIT, async {
            loop {
                let ev = self.rx.recv().await.expect("이벤트 채널이 닫힘");
                if let Some(v) = pick(&ev) {
                    return v;
                }
            }
        })
        .await
        .expect("기다리던 이벤트가 오지 않음")
    }

    async fn connected(&mut self) {
        self.wait(|e| matches!(e, Event::Connected(_)).then_some(()))
            .await
    }

    async fn unauthorized(&mut self) {
        self.wait(|e| match e {
            Event::Connected(_) => panic!("토큰 없이 연결됨"),
            Event::Unauthorized => Some(()),
            _ => None,
        })
        .await
    }
}

/// 업그레이드 요청을 직접 보내 HTTP 상태를 본다. 성공(101)이면 소켓을 돌려준다.
async fn raw_connect(
    url: &str,
    token: Option<&str>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    u16,
> {
    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(SUBPROTOCOL),
    );
    if let Some(t) = token {
        req.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {t}")).unwrap(),
        );
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(ws),
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => Err(r.status().as_u16()),
        Err(e) => panic!("연결 오류: {e}"),
    }
}

// ---------------------------------------------------------------- 시험

#[tokio::test]
async fn token_required_when_account_server_set() {
    let acc = account().await;
    let relay = relay_with(Some(auth_config(&acc))).await;
    assert!(relay.auth_ready());
    let (_, token) = acc.login("a@example.com").await;

    Peer::connect(&url(&relay), None).unauthorized().await;
    Peer::connect(&url(&relay), Some("garbage"))
        .unauthorized()
        .await;
    // 다른 키로 서명한 (모양은 맞는) 토큰
    let (forged, _) =
        Signer::from_seed(&[1u8; 32]).issue("ffffffffffffffffffffffffffffffff", 0x7fff_0000);
    Peer::connect(&url(&relay), Some(&forged))
        .unauthorized()
        .await;
    // 만료된 토큰
    let now = clip_ferry_account::now_ms() / 1000;
    let (expired, _) = acc
        .state
        .signer
        .issue("ffffffffffffffffffffffffffffffff", now - 7200);
    Peer::connect(&url(&relay), Some(&expired))
        .unauthorized()
        .await;

    let mut ok = Peer::connect(&url(&relay), Some(&token));
    ok.connected().await;
    assert!(relay.metrics().unauthorized >= 4);

    assert_eq!(raw_connect(&url(&relay), None).await.err(), Some(401));
}

#[tokio::test]
async fn relay_without_account_server_stays_open() {
    let relay = relay_with(None).await;
    Peer::connect(&url(&relay), None).connected().await;
}

#[tokio::test]
async fn not_ready_until_keys_arrive() {
    // 아무도 듣지 않는 계정 서버 주소
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = dead.local_addr().unwrap();
    drop(dead);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = spawn(
        listener,
        Config {
            auth: Some(AuthConfig::new(format!("http://{addr}"))),
            ..Config::default()
        },
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!relay.auth_ready());
    assert_eq!(raw_connect(&url(&relay), Some("x")).await.err(), Some(503));
}

#[tokio::test]
async fn delivers_and_reports_usage_per_account() {
    let acc = account().await;
    let relay = relay_with(Some(auth_config(&acc))).await;
    let (alice, ta) = acc.login("alice@example.com").await;
    let (bob, tb) = acc.login("bob@example.com").await;

    let chan = [7u8; 32];
    let mut a1 = Peer::connect(&url(&relay), Some(&ta));
    let mut a2 = Peer::connect(&url(&relay), Some(&ta));
    let mut b = Peer::connect(&url(&relay), Some(&tb));
    a1.client.subscribe(chan, [1u8; 16]);
    a2.client.subscribe(chan, [2u8; 16]);
    b.client.subscribe([8u8; 32], [3u8; 16]);
    a1.connected().await;
    a2.connected().await;
    b.connected().await;
    a2.wait(|e| match e {
        Event::Presence(p) if p.online_tags.len() == 2 => Some(()),
        _ => None,
    })
    .await;

    for msg in ["one", "two!", "three"] {
        let id = a1.client.publish(chan, msg.as_bytes().to_vec(), vec![]);
        a1.wait(|e| match e {
            Event::Ack(x) if x.request_id == id => Some(()),
            _ => None,
        })
        .await;
    }
    for want in ["one", "two!", "three"] {
        let got = a2
            .wait(|e| match e {
                Event::Deliver(d) => Some(d.payload.clone()),
                _ => None,
            })
            .await;
        assert_eq!(got, want.as_bytes());
    }

    assert_eq!(
        relay.flush_usage().await.unwrap(),
        1,
        "게시한 계정은 alice뿐"
    );
    let usage = acc.state.store.usage(&alice).await.unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].messages, 3);
    assert_eq!(usage[0].bytes, (3 + 4 + 5) as i64);
    assert!(acc.state.store.usage(&bob).await.unwrap().is_empty());
    // 한 번 보낸 것은 다시 보내지 않는다
    assert_eq!(relay.flush_usage().await.unwrap(), 0);
}

#[tokio::test]
async fn usage_kept_when_account_server_rejects() {
    let acc = account().await;
    let mut cfg = auth_config(&acc);
    cfg.usage_secret = Some(Secret("wrong".into()));
    let relay = relay_with(Some(cfg)).await;
    let (_, t) = acc.login("c@example.com").await;
    let chan = [5u8; 32];
    let mut p = Peer::connect(&url(&relay), Some(&t));
    p.client.subscribe(chan, [1u8; 16]);
    p.connected().await;
    let id = p.client.publish(chan, b"x".to_vec(), vec![]);
    p.wait(|e| match e {
        Event::Ack(x) if x.request_id == id => Some(()),
        _ => None,
    })
    .await;
    assert!(relay.flush_usage().await.is_err(), "틀린 비밀은 401");
    assert!(
        relay.flush_usage().await.is_err(),
        "되돌려 두었다가 다시 보낸다"
    );
}

#[tokio::test]
async fn account_connection_limit() {
    let acc = account().await;
    let relay = relay_with(Some(AuthConfig {
        max_conns_per_account: 1,
        ..auth_config(&acc)
    }))
    .await;
    let (_, ta) = acc.login("d@example.com").await;
    let (_, tb) = acc.login("e@example.com").await;
    let first = raw_connect(&url(&relay), Some(&ta)).await.expect("첫 연결");
    assert_eq!(raw_connect(&url(&relay), Some(&ta)).await.err(), Some(429));
    // 다른 계정은 영향 없음
    let _other = raw_connect(&url(&relay), Some(&tb))
        .await
        .expect("다른 계정");
    // 첫 연결을 닫으면 자리가 돌아온다
    drop(first);
    let mut ok = false;
    for _ in 0..50 {
        if raw_connect(&url(&relay), Some(&ta)).await.is_ok() {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "연결을 닫은 뒤에도 자리가 돌아오지 않음");
}

#[tokio::test]
async fn account_byte_limit() {
    let acc = account().await;
    let relay = relay_with(Some(AuthConfig {
        max_bytes_per_min_per_account: 3000,
        ..auth_config(&acc)
    }))
    .await;
    let (_, ta) = acc.login("f@example.com").await;
    let (_, tb) = acc.login("g@example.com").await;
    let chan = [6u8; 32];
    let mut a = Peer::connect(&url(&relay), Some(&ta));
    a.client.subscribe(chan, [1u8; 16]);
    a.connected().await;

    let first = a.client.publish(chan, vec![0u8; 2000], vec![]);
    a.wait(|e| match e {
        Event::Ack(x) if x.request_id == first => Some(()),
        Event::Error(x) if x.request_id == first => panic!("첫 게시가 막힘"),
        _ => None,
    })
    .await;
    let second = a.client.publish(chan, vec![0u8; 2000], vec![]);
    let code = a
        .wait(|e| match e {
            Event::Error(x) if x.request_id == second => Some(x.code()),
            Event::Ack(x) if x.request_id == second => panic!("한도를 넘었는데 받아들임"),
            _ => None,
        })
        .await;
    assert_eq!(code, ErrorCode::RateLimited);

    // 다른 계정은 따로 센다
    let mut b = Peer::connect(&url(&relay), Some(&tb));
    b.client.subscribe(chan, [2u8; 16]);
    b.connected().await;
    let id = b.client.publish(chan, vec![0u8; 2000], vec![]);
    b.wait(|e| match e {
        Event::Ack(x) if x.request_id == id => Some(()),
        Event::Error(x) if x.request_id == id => panic!("다른 계정까지 막힘"),
        _ => None,
    })
    .await;
}
