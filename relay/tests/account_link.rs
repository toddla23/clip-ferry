//! 계정 연결(F37, docs/03-protocol.md 4.4) 통합 테스트: 실제 릴레이로
//! 새 기기의 요청 → 기존 기기 세션이 알림 → 봉인 후 공개 페어링 → 참여까지 돌린다.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clip_ferry_core::api::{Clip, ClipFerry, Config};
use clip_ferry_core::link::{ticket_message, AccountLink, LinkRequestInfo};
use clip_ferry_core::pair_flow::{start_link_invite, start_link_join, InviteUpdate, JoinUpdate};
use clip_ferry_core::pb;
use clip_ferry_core::session::EventSink;
use clip_ferry_core::store::MemorySecretStore;
use clip_ferry_core::transport::TransportConfig;
use clip_ferry_relay::{spawn, Config as RelayConfig, RelayHandle};
use ed25519_dalek::{Signer, SigningKey};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(10);

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

async fn relay() -> (RelayHandle, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let handle = spawn(
        listener,
        RelayConfig {
            max_frames_per_sec: 1000,
            ..RelayConfig::default()
        },
    );
    let url = format!("ws://{}", handle.local_addr());
    (handle, url)
}

fn fast() -> TransportConfig {
    TransportConfig {
        backoff_initial: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        ..TransportConfig::default()
    }
}

fn server_key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

fn account(user: &str) -> AccountLink {
    AccountLink {
        user_id: user.into(),
        server_keys: vec![("kid1".into(), server_key().verifying_key().to_bytes())],
    }
}

/// 계정 서버가 해 주는 일: 새 기기의 공개키로 참여 확인서를 서명한다.
fn ticket(user: &str, p: &Profile) -> pb::LinkTicket {
    let (sign_pub, dh_pub, name) = p.app.link_identity().unwrap();
    let expires = clip_ferry_core::api::now_ms() + 10 * 60 * 1000;
    let msg = ticket_message(user, &sign_pub, &dh_pub, &name, expires).unwrap();
    pb::LinkTicket {
        user_id: user.into(),
        sign_pub: sign_pub.to_vec(),
        dh_pub: dh_pub.to_vec(),
        name,
        expires_at_ms: expires,
        kid: "kid1".into(),
        signature: server_key().sign(&msg).to_bytes().to_vec(),
    }
}

struct Sink(Mutex<mpsc::UnboundedSender<LinkRequestInfo>>);

impl EventSink for Sink {
    fn on_clip(&self, _clip: Clip) {}
    fn on_link_request(&self, request: LinkRequestInfo) {
        let _ = self.0.lock().unwrap().send(request);
    }
}

async fn next<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> T {
    timeout(WAIT, rx.recv())
        .await
        .expect("시간 초과")
        .expect("채널 닫힘")
}

async fn next_join(rx: &mut mpsc::UnboundedReceiver<JoinUpdate>) -> JoinUpdate {
    loop {
        match next(rx).await {
            JoinUpdate::Connected => continue,
            other => return other,
        }
    }
}

#[tokio::test]
async fn same_account_device_joins_after_sas_approval() {
    let (_relay, url) = relay().await;
    let a = profile("laptop", true);
    let b = profile("new-phone", false);

    // 기존 기기: 평소 세션이 계정 채널도 듣는다
    let (req_tx, mut req_rx) = mpsc::unbounded_channel();
    let session = a
        .app
        .start_session(&url, fast(), Arc::new(Sink(Mutex::new(req_tx))))
        .unwrap();
    session.set_account_link(Some(account("user-1")));
    assert!(session.wait_connected(WAIT).await);

    // 새 기기: 참여 요청
    let (b_tx, mut b_rx) = mpsc::unbounded_channel();
    let _b_flow = start_link_join(&b.app, &url, fast(), ticket("user-1", &b), move |u| {
        let _ = b_tx.send(u);
    })
    .unwrap();

    let request = next(&mut req_rx).await;
    assert_eq!(request.name, "new-phone");

    // 사용자가 기존 기기에서 "연결"을 눌렀다
    let (a_tx, mut a_rx) = mpsc::unbounded_channel();
    let a_flow = start_link_invite(
        &a.app,
        &url,
        fast(),
        account("user-1").channel(),
        &request,
        move |u| {
            let _ = a_tx.send(u);
        },
    )
    .unwrap();

    let JoinUpdate::WaitingApproval {
        inviter_name,
        sas: b_sas,
    } = next_join(&mut b_rx).await
    else {
        panic!("WaitingApproval 없음");
    };
    assert_eq!(inviter_name, "laptop");
    let InviteUpdate::Request {
        joiner_name,
        sas: a_sas,
    } = next(&mut a_rx).await
    else {
        panic!("Request 없음");
    };
    assert_eq!(joiner_name, "new-phone");
    assert_eq!(a_sas, b_sas, "양쪽 확인 숫자가 같아야 한다");

    a_flow.decide(true);
    assert_eq!(next_join(&mut b_rx).await, JoinUpdate::Joined);
    assert!(matches!(
        next(&mut a_rx).await,
        InviteUpdate::Approved {
            delivered: true,
            ..
        }
    ));
    assert_eq!(a.app.devices().len(), 2);
    assert_eq!(b.app.devices().len(), 2);
    assert_eq!(
        a.app.connection().unwrap().channel_id,
        b.app.connection().unwrap().channel_id
    );

    // 같은 요청은 한 번만 알린다 (새 기기는 응답 전까지 3초마다 다시 보냈다)
    assert!(req_rx.try_recv().is_err());
}

#[tokio::test]
async fn other_account_request_is_not_shown() {
    let (_relay, url) = relay().await;
    let a = profile("laptop", true);
    let b = profile("stranger", false);

    let (req_tx, mut req_rx) = mpsc::unbounded_channel();
    let session = a
        .app
        .start_session(&url, fast(), Arc::new(Sink(Mutex::new(req_tx))))
        .unwrap();
    session.set_account_link(Some(account("user-1")));
    assert!(session.wait_connected(WAIT).await);

    // 다른 계정(user-2)의 확인서를 user-1의 계정 채널에 올린다
    let mut forged = ticket("user-2", &b);
    forged.user_id = "user-1".into(); // 서명과 맞지 않는다
    let (b_tx, _b_rx) = mpsc::unbounded_channel();
    let _b1 = start_link_join(&b.app, &url, fast(), forged, move |u| {
        let _ = b_tx.send(u);
    })
    .unwrap();
    // 서명은 맞지만 다른 계정: user-2 채널로 가므로 user-1 기기에는 닿지 않는다
    let (b_tx, _b_rx) = mpsc::unbounded_channel();
    let _b2 = start_link_join(&b.app, &url, fast(), ticket("user-2", &b), move |u| {
        let _ = b_tx.send(u);
    })
    .unwrap();

    assert!(
        timeout(Duration::from_secs(4), req_rx.recv())
            .await
            .is_err(),
        "다른 계정의 요청이 알려지면 안 된다"
    );

    // 로그아웃하면 더 듣지 않는다
    session.set_account_link(None);
}

#[tokio::test]
async fn link_join_refuses_when_already_in_group() {
    let (_relay, url) = relay().await;
    let a = profile("laptop", true);
    let t = ticket("user-1", &a);
    assert!(start_link_join(&a.app, &url, fast(), t, |_| {}).is_err());
}
