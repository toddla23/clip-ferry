//! 릴레이와 코어 전송 계층을 실제 소켓으로 붙여 보는 통합 테스트.

use std::net::SocketAddr;
use std::time::Duration;

use clip_ferry_core::pb::{self, relay_frame, ErrorCode};
use clip_ferry_core::transport::{Client, Event, TransportConfig, SUBPROTOCOL};
use clip_ferry_relay::{spawn, Config, RelayHandle};
use futures_util::StreamExt;
use prost::Message as _;
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const WAIT: Duration = Duration::from_secs(5);

async fn start(config: Config) -> RelayHandle {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    spawn(listener, config)
}

async fn start_on(addr: SocketAddr) -> RelayHandle {
    for _ in 0..100 {
        if let Ok(listener) = TcpListener::bind(addr).await {
            return spawn(listener, Config::default());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("포트를 다시 열 수 없음");
}

fn fast() -> TransportConfig {
    TransportConfig {
        backoff_initial: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        ..TransportConfig::default()
    }
}

fn url(relay: &RelayHandle) -> String {
    format!("ws://{}", relay.local_addr())
}

fn chan(n: u8) -> [u8; 32] {
    [n; 32]
}

fn tag(n: u8) -> [u8; 16] {
    [n; 16]
}

struct Peer {
    client: Client,
    rx: UnboundedReceiver<Event>,
}

impl Peer {
    fn connect(url: &str) -> Peer {
        let (client, rx) = Client::connect(url, fast());
        Peer { client, rx }
    }

    /// 조건에 맞는 이벤트가 올 때까지 기다린다. 다른 이벤트는 버린다.
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

    async fn presence(&mut self, online: usize) {
        self.wait(|e| match e {
            Event::Presence(p) if p.online_tags.len() == online => Some(()),
            _ => None,
        })
        .await
    }

    async fn ack(&mut self, request_id: u32) -> pb::Ack {
        self.wait(|e| match e {
            Event::Ack(a) if a.request_id == request_id => Some(*a),
            _ => None,
        })
        .await
    }

    async fn error_code(&mut self) -> ErrorCode {
        self.wait(|e| match e {
            Event::Error(err) => Some(err.code()),
            _ => None,
        })
        .await
    }

    async fn deliver(&mut self) -> pb::Deliver {
        self.wait(|e| match e {
            Event::Deliver(d) => Some(d.clone()),
            _ => None,
        })
        .await
    }

    /// 짧은 시간 안에 `Deliver`가 오지 않는지 확인한다.
    async fn expect_no_deliver(&mut self) {
        let end = tokio::time::Instant::now() + Duration::from_millis(300);
        while let Ok(Some(ev)) = tokio::time::timeout_at(end, self.rx.recv()).await {
            assert!(
                !matches!(ev, Event::Deliver(_)),
                "받으면 안 되는 Deliver: {ev:?}"
            );
        }
    }
}

/// A, B가 같은 채널에 구독한 상태를 만든다.
async fn pair(relay: &RelayHandle, channel: u8) -> (Peer, Peer) {
    let mut a = Peer::connect(&url(relay));
    a.client.subscribe(chan(channel), tag(1));
    a.presence(1).await;
    let mut b = Peer::connect(&url(relay));
    b.client.subscribe(chan(channel), tag(2));
    b.presence(2).await;
    a.presence(2).await;
    (a, b)
}

#[tokio::test]
async fn delivers_to_other_subscriber_on_same_channel() {
    let relay = start(Config::default()).await;
    let (mut a, mut b) = pair(&relay, 7).await;

    let id = a.client.publish(chan(7), b"hello".to_vec(), vec![]);
    let d = b.deliver().await;
    assert_eq!(d.payload, b"hello");
    assert_eq!(d.channel_id, chan(7));
    assert_eq!(d.from_tag, tag(1));
    assert_eq!(a.ack(id).await.delivered, 1);

    // 보낸 사람에게는 되돌아오지 않는다.
    a.expect_no_deliver().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn does_not_deliver_to_other_channel() {
    let relay = start(Config::default()).await;
    let (mut a, _b) = pair(&relay, 7).await;
    let mut c = Peer::connect(&url(&relay));
    c.client.subscribe(chan(8), tag(3));
    c.presence(1).await;

    let id = a.client.publish(chan(7), b"secret".to_vec(), vec![]);
    assert_eq!(a.ack(id).await.delivered, 1); // B만
    c.expect_no_deliver().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn to_filter_targets_only_listed_tags() {
    let relay = start(Config::default()).await;
    let (mut a, mut b) = pair(&relay, 7).await;
    let mut c = Peer::connect(&url(&relay));
    c.client.subscribe(chan(7), tag(3));
    c.presence(3).await;

    let id = a.client.publish(chan(7), b"only-c".to_vec(), vec![tag(3)]);
    assert_eq!(c.deliver().await.payload, b"only-c");
    assert_eq!(a.ack(id).await.delivered, 1);
    b.expect_no_deliver().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn publish_without_subscription_is_rejected() {
    let relay = start(Config::default()).await;
    let mut a = Peer::connect(&url(&relay));
    a.wait(|e| matches!(e, Event::Connected(_)).then_some(()))
        .await;
    a.client.publish(chan(7), b"x".to_vec(), vec![]);
    assert_eq!(a.error_code().await, ErrorCode::NotSubscribed);
    relay.shutdown().await;
}

#[tokio::test]
async fn oversized_frame_gets_too_large() {
    let relay = start(Config {
        max_frame_bytes: 1024,
        ..Config::default()
    })
    .await;
    let mut a = Peer::connect(&url(&relay));
    a.client.subscribe(chan(7), tag(1));
    a.presence(1).await;

    let id = a.client.publish(chan(7), vec![0u8; 1500], vec![]);
    let err = a
        .wait(|e| match e {
            Event::Error(err) if err.request_id == id => Some(err.code()),
            _ => None,
        })
        .await;
    assert_eq!(err, ErrorCode::TooLarge);

    // 연결은 유지되고 정상 프레임은 계속 처리된다.
    let id = a.client.publish(chan(7), vec![0u8; 10], vec![]);
    assert_eq!(a.ack(id).await.delivered, 0);
    relay.shutdown().await;
}

#[tokio::test]
async fn too_fast_gets_rate_limited() {
    let relay = start(Config {
        max_frames_per_sec: 5,
        ..Config::default()
    })
    .await;
    let mut a = Peer::connect(&url(&relay));
    a.client.subscribe(chan(7), tag(1));
    a.presence(1).await;

    for _ in 0..30 {
        a.client.publish(chan(7), b"x".to_vec(), vec![]);
    }
    assert_eq!(a.error_code().await, ErrorCode::RateLimited);
    relay.shutdown().await;
}

#[tokio::test]
async fn no_receiver_means_delivered_zero() {
    let relay = start(Config::default()).await;
    let mut a = Peer::connect(&url(&relay));
    a.client.subscribe(chan(7), tag(1));
    a.presence(1).await;

    let id = a.client.publish(chan(7), b"nobody".to_vec(), vec![]);
    let ack = a.ack(id).await;
    assert_eq!(ack.delivered, 0);
    assert_eq!(ack.pushed, 0);
    relay.shutdown().await;
}

#[tokio::test]
async fn too_many_channels_is_rejected() {
    let relay = start(Config {
        max_channels_per_conn: 2,
        ..Config::default()
    })
    .await;
    let mut a = Peer::connect(&url(&relay));
    a.client.subscribe(chan(1), tag(1));
    a.client.subscribe(chan(2), tag(1));
    a.client.subscribe(chan(3), tag(1));
    assert_eq!(a.error_code().await, ErrorCode::TooManyChannels);
    relay.shutdown().await;
}

#[tokio::test]
async fn presence_follows_membership() {
    let relay = start(Config::default()).await;
    let (mut a, b) = pair(&relay, 7).await;
    drop(b); // 연결이 닫히면 구독도 사라진다.
    a.presence(1).await;
    relay.shutdown().await;
}

#[tokio::test]
async fn hello_carries_limits() {
    let relay = start(Config::default()).await;
    let mut a = Peer::connect(&url(&relay));
    let hello = a
        .wait(|e| match e {
            Event::Connected(h) => Some(h.clone()),
            _ => None,
        })
        .await;
    let limits = hello.limits.unwrap();
    assert_eq!(limits.max_frame_bytes, 2 * 1024 * 1024);
    assert_eq!(limits.max_channels_per_conn, 8);
    relay.shutdown().await;
}

#[tokio::test]
async fn too_many_connections_per_ip_is_refused() {
    let relay = start(Config {
        max_conns_per_ip: 2,
        ..Config::default()
    })
    .await;
    let connect = || async {
        let mut req = url(&relay).into_client_request().unwrap();
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static(SUBPROTOCOL),
        );
        tokio_tungstenite::connect_async(req).await
    };
    let _first = connect().await.unwrap();
    let _second = connect().await.unwrap();
    assert!(connect().await.is_err());
    relay.shutdown().await;
}

#[tokio::test]
async fn missing_subprotocol_gets_unsupported_version() {
    let relay = start(Config::default()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url(&relay)).await.unwrap();
    let Some(Ok(Message::Binary(bytes))) = ws.next().await else {
        panic!("에러 프레임이 오지 않음");
    };
    let frame = pb::RelayFrame::decode(bytes.as_slice()).unwrap();
    let Some(relay_frame::Kind::Error(e)) = frame.kind else {
        panic!("에러가 아님");
    };
    assert_eq!(e.code(), ErrorCode::UnsupportedVersion);
    relay.shutdown().await;
}

#[tokio::test]
async fn client_reconnects_and_resubscribes_after_relay_restart() {
    let relay = start(Config::default()).await;
    let addr = relay.local_addr();
    let mut a = Peer::connect(&url(&relay));
    a.client.subscribe(chan(7), tag(1));
    a.presence(1).await;

    relay.shutdown().await;
    a.wait(|e| matches!(e, Event::Disconnected).then_some(()))
        .await;
    a.wait(|e| matches!(e, Event::Reconnecting { .. }).then_some(()))
        .await;

    let relay = start_on(addr).await;
    a.wait(|e| matches!(e, Event::Connected(_)).then_some(()))
        .await;
    a.presence(1).await; // 재구독됨

    let mut b = Peer::connect(&url(&relay));
    b.client.subscribe(chan(7), tag(2));
    b.presence(2).await;
    let id = b.client.publish(chan(7), b"after restart".to_vec(), vec![]);
    assert_eq!(a.deliver().await.payload, b"after restart");
    assert_eq!(b.ack(id).await.delivered, 1);
    relay.shutdown().await;
}

#[tokio::test]
async fn publish_while_disconnected_is_reported_as_failed() {
    let relay = start(Config::default()).await;
    let mut a = Peer::connect(&url(&relay));
    a.wait(|e| matches!(e, Event::Connected(_)).then_some(()))
        .await;
    relay.shutdown().await;
    a.wait(|e| matches!(e, Event::Reconnecting { .. }).then_some(()))
        .await;

    let id = a.client.publish(chan(7), b"lost".to_vec(), vec![]);
    let failed = a
        .wait(|e| match e {
            Event::PublishFailed { request_id } => Some(*request_id),
            _ => None,
        })
        .await;
    assert_eq!(failed, id);
}
