//! 텍스트 송수신 통합 테스트: 실제 릴레이와 `Session`으로 세 프로필이 주고받는다.

use std::sync::Arc;
use std::time::Duration;

use clip_ferry_core::api::{now_ms, Clip, ClipFerry, Config, Incoming};
use clip_ferry_core::envelope::{Source, MAX_TEXT_BYTES};
use clip_ferry_core::pairing::{InviterEvent, JoinerEvent};
use clip_ferry_core::pb::{self, client_frame};
use clip_ferry_core::session::{EventSink, Session};
use clip_ferry_core::store::MemorySecretStore;
use clip_ferry_core::transport::{TransportConfig, SUBPROTOCOL};
use clip_ferry_relay::{spawn, Config as RelayConfig, RelayHandle};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const WAIT: Duration = Duration::from_secs(5);

struct Profile {
    app: Arc<ClipFerry>,
    _dir: tempfile::TempDir,
}

fn profile(name: &str, group: bool) -> Profile {
    let dir = tempfile::tempdir().unwrap();
    let app = ClipFerry::open(
        Config {
            data_dir: dir.path().to_path_buf(),
        },
        Box::new(MemorySecretStore::default()),
    )
    .unwrap();
    app.init_device(name).unwrap();
    if group {
        app.create_group().unwrap();
    }
    Profile {
        app: Arc::new(app),
        _dir: dir,
    }
}

/// `joiner`를 `inviter`의 그룹에 넣고 `RosterUpdate` 페이로드를 돌려준다.
fn pair(inviter: &Profile, joiner: &Profile) -> Vec<u8> {
    let mut inv = inviter.app.create_invite(None).unwrap();
    let mut j = joiner.app.join(inv.invite().clone()).unwrap();
    let InviterEvent::Offer { payload, .. } = inv.handle(&j.hello(), now_ms()) else {
        panic!("Offer 없음");
    };
    assert!(matches!(j.handle(&payload), JoinerEvent::Offer { .. }));
    let approval = inviter.app.approve_join(&mut inv, now_ms()).unwrap();
    let JoinerEvent::Joined(joined) = j.handle(&approval.welcome) else {
        panic!("참여 실패");
    };
    joiner.app.accept_join(joined).unwrap();
    approval.roster_update
}

struct Collect(mpsc::UnboundedSender<Clip>);

impl EventSink for Collect {
    fn on_clip(&self, clip: Clip) {
        let _ = self.0.send(clip);
    }
}

struct Online {
    session: Session,
    clips: mpsc::UnboundedReceiver<Clip>,
}

fn fast() -> TransportConfig {
    TransportConfig {
        backoff_initial: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        ..TransportConfig::default()
    }
}

async fn relay() -> RelayHandle {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    spawn(
        listener,
        RelayConfig {
            max_frames_per_sec: 1000,
            ..RelayConfig::default()
        },
    )
}

fn go_online(relay: &RelayHandle, p: &Profile) -> Online {
    let (tx, clips) = mpsc::unbounded_channel();
    let session = p
        .app
        .start_session(
            &format!("ws://{}", relay.local_addr()),
            fast(),
            Arc::new(Collect(tx)),
        )
        .unwrap();
    Online { session, clips }
}

async fn expect_clip(o: &mut Online) -> Clip {
    timeout(WAIT, o.clips.recv())
        .await
        .expect("텍스트가 오지 않음")
        .expect("채널이 닫힘")
}

async fn expect_no_clip(o: &mut Online) {
    assert!(
        timeout(Duration::from_millis(300), o.clips.recv())
            .await
            .is_err(),
        "받으면 안 되는 텍스트가 옴"
    );
}

async fn ready(all: &[&Online]) {
    for o in all {
        assert!(o.session.wait_connected(WAIT).await, "연결 안 됨");
        assert!(
            o.session.wait_online(all.len(), WAIT).await,
            "구성원이 다 붙지 않음"
        );
    }
}

#[tokio::test]
async fn one_sends_the_other_two_receive() {
    let relay = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone", false);
    let c = profile("tablet", false);
    pair(&a, &b);
    let update_for_b = pair(&a, &c);
    // B가 온라인일 때 A가 보낸 RosterUpdate를 받았다고 가정한다.
    assert_eq!(
        b.app.handle_deliver(&update_for_b, Source::Relay, now_ms()),
        Ok(Incoming::DevicesChanged)
    );

    let (mut oa, mut ob, mut oc) = (
        go_online(&relay, &a),
        go_online(&relay, &b),
        go_online(&relay, &c),
    );
    ready(&[&oa, &ob, &oc]).await;

    assert_eq!(oa.session.send_text("안녕 clip-ferry").await.unwrap(), 2);
    for o in [&mut ob, &mut oc] {
        let clip = expect_clip(o).await;
        assert_eq!(clip.text, "안녕 clip-ferry");
        assert_eq!(clip.from_name, "laptop");
    }
    expect_no_clip(&mut oa).await;

    // 반대로 C가 보내면 A, B가 받는다 (B는 로스터 갱신으로 C를 안다).
    assert_eq!(oc.session.send_text("from tablet").await.unwrap(), 2);
    for o in [&mut oa, &mut ob] {
        let clip = expect_clip(o).await;
        assert_eq!(clip.text, "from tablet");
        assert_eq!(clip.from_name, "tablet");
    }
    relay.shutdown().await;
}

#[tokio::test]
async fn another_group_does_not_receive() {
    let relay = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let outsider = profile("stranger", true);

    let (oa, mut ob, mut oo) = (
        go_online(&relay, &a),
        go_online(&relay, &b),
        go_online(&relay, &outsider),
    );
    ready(&[&oa, &ob]).await;
    assert!(oo.session.wait_connected(WAIT).await);

    assert_eq!(oa.session.send_text("private").await.unwrap(), 1);
    assert_eq!(expect_clip(&mut ob).await.text, "private");
    expect_no_clip(&mut oo).await;
    relay.shutdown().await;
}

/// 구독자의 눈에 보이는 릴레이 전달 바이트를 모은다.
async fn observe(
    relay: &RelayHandle,
    channel: [u8; 32],
) -> (
    mpsc::UnboundedReceiver<Vec<u8>>,
    impl SinkExt<Message> + Unpin,
) {
    let mut req = format!("ws://{}", relay.local_addr())
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(SUBPROTOCOL),
    );
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let (mut sink, mut stream) = ws.split();
    let subscribe = pb::ClientFrame {
        kind: Some(client_frame::Kind::Subscribe(pb::Subscribe {
            channel_id: channel.to_vec(),
            device_tag: vec![0xEE; 16],
        })),
    };
    sink.send(Message::Binary(subscribe.encode_to_vec()))
        .await
        .ok()
        .unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            if let Message::Binary(b) = msg {
                let _ = tx.send(b);
            }
        }
    });
    (rx, sink)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn relay_never_sees_plaintext() {
    let relay = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let channel = a.app.connection().unwrap().channel_id.0;
    let (mut seen, _keep_open) = observe(&relay, channel).await;
    // 대조군을 게시할 두 번째 연결 (릴레이는 보낸 연결에는 되돌려 주지 않는다)
    let (_unused_rx, mut raw_sink) = observe(&relay, channel).await;

    let (oa, mut ob) = (go_online(&relay, &a), go_online(&relay, &b));
    ready(&[&oa, &ob]).await;
    assert!(
        oa.session.wait_online(4, WAIT).await,
        "관찰자가 구독하지 않음"
    );
    let plaintext = "TOP-SECRET 비밀 문장 🔐 clip-ferry";
    assert_eq!(oa.session.send_text(plaintext).await.unwrap(), 3);
    assert_eq!(expect_clip(&mut ob).await.text, plaintext);

    // 대조군: 암호화하지 않은 바이트를 같은 방식으로 보내면 관찰자에게 그대로 보인다.
    // (관찰 방법이 실제로 평문을 찾아낼 수 있다는 증거)
    let control = "CONTROL-PLAINTEXT-MARKER";
    let publish = pb::ClientFrame {
        kind: Some(client_frame::Kind::Publish(pb::Publish {
            request_id: 1,
            channel_id: channel.to_vec(),
            payload: control.as_bytes().to_vec(),
            to: vec![],
            push: vec![],
        })),
    };
    raw_sink
        .send(Message::Binary(publish.encode_to_vec()))
        .await
        .ok()
        .unwrap();

    let mut all = Vec::new();
    let mut saw_control = false;
    while let Ok(Some(bytes)) = timeout(Duration::from_millis(500), seen.recv()).await {
        saw_control |= contains(&bytes, control.as_bytes());
        all.extend_from_slice(&bytes);
    }
    assert!(
        saw_control,
        "관찰 방법이 평문을 찾지 못함 (테스트가 무의미)"
    );
    assert!(all.len() > 100, "암호문을 받지 못함");
    for needle in ["TOP-SECRET", "비밀 문장", "🔐", "clip-ferry", "laptop"] {
        assert!(
            !contains(&all, needle.as_bytes()),
            "릴레이가 본 바이트에 평문 '{needle}'이 있음"
        );
    }
    relay.shutdown().await;
}

#[tokio::test]
async fn unicode_and_one_mebibyte_over_the_relay() {
    let relay = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let (oa, mut ob) = (go_online(&relay, &a), go_online(&relay, &b));
    ready(&[&oa, &ob]).await;

    for text in ["한글 テスト", "😀👨‍👩‍👧‍👦🏳️‍🌈", "줄1\n줄2\r\n\t탭"]
    {
        assert_eq!(oa.session.send_text(text).await.unwrap(), 1);
        assert_eq!(expect_clip(&mut ob).await.text, text);
    }

    let at_limit = "x".repeat(MAX_TEXT_BYTES);
    assert_eq!(oa.session.send_text(&at_limit).await.unwrap(), 1);
    assert_eq!(expect_clip(&mut ob).await.text.len(), MAX_TEXT_BYTES);

    let err = oa.session.send_text(&(at_limit + "x")).await.unwrap_err();
    assert!(err.to_string().contains("너무 큽니다"), "{err}");
    expect_no_clip(&mut ob).await;
    relay.shutdown().await;
}

#[tokio::test]
async fn sending_without_connection_fails_cleanly() {
    let a = profile("laptop", true);
    let (tx, _rx) = mpsc::unbounded_channel();
    // 아무도 듣지 않는 주소
    let session = a
        .app
        .start_session("ws://127.0.0.1:1", fast(), Arc::new(Collect(tx)))
        .unwrap();
    assert!(!session.wait_connected(Duration::from_millis(300)).await);
    assert!(session.send_text("hi").await.is_err());
}
