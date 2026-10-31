//! 페어링 진행(`pair_flow`) 통합 테스트: 실제 릴레이로 초대·참여·승인·거절을 끝까지 돌린다.

use std::sync::Arc;
use std::time::Duration;

use clip_ferry_core::api::{ClipFerry, Config};
use clip_ferry_core::pair_flow::{
    start_invite, start_join, InviteFlow, InviteUpdate, JoinFlow, JoinUpdate, StartError,
};
use clip_ferry_core::store::MemorySecretStore;
use clip_ferry_core::transport::TransportConfig;
use clip_ferry_relay::{spawn, Config as RelayConfig, RelayHandle};
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

fn invite(p: &Profile, url: &str) -> (InviteFlow, mpsc::UnboundedReceiver<InviteUpdate>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let flow = start_invite(&p.app, url, fast(), move |u| {
        let _ = tx.send(u);
    })
    .unwrap();
    (flow, rx)
}

fn join(
    p: &Profile,
    code: &str,
    url: &str,
) -> Result<(JoinFlow, mpsc::UnboundedReceiver<JoinUpdate>), StartError> {
    let (tx, rx) = mpsc::unbounded_channel();
    let flow = start_join(&p.app, code, None, url, fast(), move |u| {
        let _ = tx.send(u);
    })?;
    Ok((flow, rx))
}

async fn next<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> T {
    timeout(WAIT, rx.recv())
        .await
        .expect("시간 초과")
        .expect("채널 닫힘")
}

/// `Connected`는 건너뛰고 다음 갱신을 돌려준다.
async fn next_join(rx: &mut mpsc::UnboundedReceiver<JoinUpdate>) -> JoinUpdate {
    loop {
        match next(rx).await {
            JoinUpdate::Connected => continue,
            other => return other,
        }
    }
}

#[tokio::test]
async fn approve_adds_the_device_on_both_sides() {
    let (_relay, url) = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone-sim", false);

    let (a_flow, mut a_rx) = invite(&a, &url);
    let (_b_flow, mut b_rx) = join(&b, &a_flow.text_code, &url).unwrap();

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
    assert_eq!(joiner_name, "phone-sim");
    assert_eq!(a_sas, b_sas, "양쪽 화면의 확인 숫자가 같아야 한다");
    assert_eq!(a_sas.len(), 6);

    a_flow.decide(true);
    assert_eq!(next_join(&mut b_rx).await, JoinUpdate::Joined);
    assert_eq!(
        next(&mut a_rx).await,
        InviteUpdate::Approved {
            joiner_name: "phone-sim".into(),
            delivered: true
        }
    );
    assert_eq!(a.app.devices().len(), 2);
    assert_eq!(b.app.devices().len(), 2);
    assert_eq!(b.app.status().unwrap().epoch, Some(1));
}

#[tokio::test]
async fn reject_ends_both_sides_without_adding() {
    let (_relay, url) = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone-sim", false);

    let (a_flow, mut a_rx) = invite(&a, &url);
    let (_b_flow, mut b_rx) = join(&b, &a_flow.text_code, &url).unwrap();
    assert!(matches!(
        next_join(&mut b_rx).await,
        JoinUpdate::WaitingApproval { .. }
    ));
    assert!(matches!(
        next(&mut a_rx).await,
        InviteUpdate::Request { .. }
    ));

    a_flow.decide(false);
    assert_eq!(next(&mut a_rx).await, InviteUpdate::Rejected);
    let JoinUpdate::Ended(msg) = next_join(&mut b_rx).await else {
        panic!("Ended 없음");
    };
    assert!(msg.contains("거절"), "{msg}");
    assert_eq!(a.app.devices().len(), 1);
    assert_eq!(b.app.status().unwrap().epoch, None);
}

#[tokio::test]
async fn wrong_codes_and_existing_groups_are_refused_up_front() {
    let (_relay, url) = relay().await;
    let a = profile("laptop", true);
    let (a_flow, _a_rx) = invite(&a, &url);

    let fresh = profile("fresh", false);
    // 한 글자를 바꾼 코드는 체크 기호로 걸러진다.
    let mut bad = a_flow.text_code.clone().into_bytes();
    let i = bad.iter().position(|c| c.is_ascii_alphanumeric()).unwrap();
    bad[i] = if bad[i] == b'0' { b'1' } else { b'0' };
    let bad = String::from_utf8(bad).unwrap();
    assert!(matches!(
        join(&fresh, &bad, &url),
        Err(StartError::Invite(_))
    ));
    assert!(matches!(
        join(&fresh, "쓰레기", &url),
        Err(StartError::Invite(_))
    ));

    // 이미 그룹이 있는 기기는 참여할 수 없다.
    let member = profile("member", true);
    assert!(matches!(
        join(&member, &a_flow.text_code, &url),
        Err(StartError::App(_))
    ));
}

#[tokio::test]
async fn a_second_joiner_invalidates_the_invite() {
    let (_relay, url) = relay().await;
    let a = profile("laptop", true);
    let b = profile("first", false);
    let c = profile("second", false);

    let (a_flow, mut a_rx) = invite(&a, &url);
    let (_b_flow, mut b_rx) = join(&b, &a_flow.text_code, &url).unwrap();
    assert!(matches!(
        next_join(&mut b_rx).await,
        JoinUpdate::WaitingApproval { .. }
    ));
    assert!(matches!(
        next(&mut a_rx).await,
        InviteUpdate::Request { .. }
    ));

    let (_c_flow, _c_rx) = join(&c, &a_flow.text_code, &url).unwrap();
    let InviteUpdate::Ended(msg) = next(&mut a_rx).await else {
        panic!("Ended 없음");
    };
    assert!(msg.contains("무효"), "{msg}");
    assert_eq!(a.app.devices().len(), 1);
}

#[tokio::test]
async fn dropping_the_invite_closes_it() {
    let (_relay, url) = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone-sim", false);

    let (a_flow, a_rx) = invite(&a, &url);
    let code = a_flow.text_code.clone();
    drop(a_flow);
    drop(a_rx);

    let (_b_flow, mut b_rx) = join(&b, &code, &url).unwrap();
    // 초대한 쪽이 없으므로 응답이 오지 않는다.
    let waited = timeout(Duration::from_millis(1500), async {
        loop {
            if let JoinUpdate::WaitingApproval { .. } = next(&mut b_rx).await {
                return;
            }
        }
    })
    .await;
    assert!(waited.is_err());
    assert_eq!(a.app.devices().len(), 1);
}
