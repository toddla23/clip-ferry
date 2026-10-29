//! 기기 제거·키 회전·복구 통합 테스트: 실제 릴레이와 `Session`으로 확인한다.

use std::sync::Arc;
use std::time::Duration;

use clip_ferry_core::api::{now_ms, Clip, ClipFerry, Config, GroupHealth};
use clip_ferry_core::pairing::{InviterEvent, JoinerEvent};
use clip_ferry_core::session::{EventSink, Session};
use clip_ferry_core::store::MemorySecretStore;
use clip_ferry_core::transport::TransportConfig;
use clip_ferry_relay::{spawn, Config as RelayConfig, RelayHandle};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

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

/// A가 B, C와 한 그룹이고 모두 서로의 로스터를 안다.
fn trio() -> (Profile, Profile, Profile) {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    let c = profile("tablet", false);
    pair(&a, &b);
    let update_for_b = pair(&a, &c);
    b.app
        .handle_deliver(
            &update_for_b,
            clip_ferry_core::envelope::Source::Relay,
            now_ms(),
        )
        .unwrap();
    (a, b, c)
}

#[derive(Debug)]
enum Ev {
    Clip(Clip),
    Epoch(u64),
    Health(GroupHealth),
}

struct Collect(mpsc::UnboundedSender<Ev>);

impl EventSink for Collect {
    fn on_clip(&self, clip: Clip) {
        let _ = self.0.send(Ev::Clip(clip));
    }
    fn on_epoch_changed(&self, epoch: u64) {
        let _ = self.0.send(Ev::Epoch(epoch));
    }
    fn on_group_health(&self, health: GroupHealth) {
        let _ = self.0.send(Ev::Health(health));
    }
}

struct Online {
    session: Session,
    events: mpsc::UnboundedReceiver<Ev>,
}

fn go_online(relay: &RelayHandle, p: &Profile) -> Online {
    let (tx, events) = mpsc::unbounded_channel();
    let session = p
        .app
        .start_session(
            &format!("ws://{}", relay.local_addr()),
            TransportConfig {
                backoff_initial: Duration::from_millis(50),
                backoff_max: Duration::from_millis(200),
                ..TransportConfig::default()
            },
            Arc::new(Collect(tx)),
        )
        .unwrap();
    Online { session, events }
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

impl Online {
    /// 조건에 맞는 이벤트를 기다린다. 다른 이벤트는 건너뛴다.
    async fn wait<T>(&mut self, mut pick: impl FnMut(&Ev) -> Option<T>) -> T {
        timeout(WAIT, async {
            loop {
                let ev = self.events.recv().await.expect("이벤트 채널이 닫힘");
                if let Some(v) = pick(&ev) {
                    return v;
                }
            }
        })
        .await
        .expect("기다리던 이벤트가 오지 않음")
    }

    async fn clip(&mut self) -> Clip {
        self.wait(|e| match e {
            Ev::Clip(c) => Some(c.clone()),
            _ => None,
        })
        .await
    }

    async fn epoch(&mut self, epoch: u64) {
        self.wait(|e| matches!(e, Ev::Epoch(n) if *n == epoch).then_some(()))
            .await
    }

    async fn health(&mut self, health: GroupHealth) {
        self.wait(|e| matches!(e, Ev::Health(h) if *h == health).then_some(()))
            .await
    }

    async fn no_clip(&mut self) {
        let end = tokio::time::Instant::now() + Duration::from_millis(400);
        while let Ok(Some(ev)) = tokio::time::timeout_at(end, self.events.recv()).await {
            assert!(!matches!(ev, Ev::Clip(_)), "받으면 안 되는 텍스트: {ev:?}");
        }
    }
}

async fn all_ready(all: &[&Online]) {
    for o in all {
        assert!(o.session.wait_connected(WAIT).await, "연결 안 됨");
        assert!(o.session.wait_online(all.len(), WAIT).await, "다 붙지 않음");
    }
}

#[tokio::test]
async fn removed_device_stops_receiving_and_online_members_follow() {
    let relay = relay().await;
    let (a, b, c) = trio();
    let (oa, mut ob, mut oc) = (
        go_online(&relay, &a),
        go_online(&relay, &b),
        go_online(&relay, &c),
    );
    all_ready(&[&oa, &ob, &oc]).await;

    // 제거 시점에 온라인이던 B와 C가 모두 Rekey를 받는다.
    assert_eq!(oa.session.remove_device("phone").await.unwrap(), 2);
    ob.health(GroupHealth::Removed).await;
    oc.epoch(2).await;

    // 새 채널에서는 A와 C만 오간다.
    assert!(oa.session.wait_online(2, WAIT).await);
    assert_eq!(oa.session.send_text("after remove").await.unwrap(), 1);
    assert_eq!(oc.clip().await.text, "after remove");
    ob.no_clip().await;

    // 제거된 B는 이제 세션을 시작할 수 없다.
    assert!(b
        .app
        .start_session(
            "ws://127.0.0.1:1",
            TransportConfig::default(),
            Arc::new(Collect(mpsc::unbounded_channel().0))
        )
        .is_err());
    assert_eq!(a.app.devices().len(), 2);
    relay.shutdown().await;
}

#[tokio::test]
async fn offline_member_catches_up_when_it_reconnects() {
    let relay = relay().await;
    let (a, b, c) = trio();

    // C와 B는 꺼져 있고 A만 있는 상태에서 B를 제거한다.
    let oa = go_online(&relay, &a);
    assert!(oa.session.wait_connected(WAIT).await);
    oa.session.remove_device("phone").await.unwrap();
    assert_eq!(c.app.status().unwrap().epoch, Some(1));

    // C가 돌아오면 연결 직후 질의하고, A가 최신 키로 응답한다.
    let mut oc = go_online(&relay, &c);
    oc.epoch(2).await;
    assert_eq!(c.app.status().unwrap().epoch, Some(2));
    assert!(oc.session.wait_online(2, WAIT).await);
    assert!(oa.session.wait_online(2, WAIT).await);

    assert_eq!(oa.session.send_text("welcome back").await.unwrap(), 1);
    assert_eq!(oc.clip().await.text, "welcome back");

    // B도 돌아오면 자기가 제거된 것을 안다.
    let mut ob = go_online(&relay, &b);
    ob.health(GroupHealth::Removed).await;
    assert_eq!(b.app.group_health(now_ms()), GroupHealth::Removed);
    ob.no_clip().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn devices_already_waiting_are_caught_up_when_the_remover_returns() {
    let relay = relay().await;
    let (a, b, c) = trio();

    // A가 오프라인인 상태에서 제거를 수행하고 끝난다(한 번만 접속하는 CLI처럼).
    {
        let oa = go_online(&relay, &a);
        assert!(oa.session.wait_connected(WAIT).await);
        oa.session.remove_device("phone").await.unwrap();
    }
    // B와 C는 이전 에포크 그대로 접속해서 기다린다. 질의에 답할 기기는 없다.
    let mut ob = go_online(&relay, &b);
    let mut oc = go_online(&relay, &c);
    assert!(ob.session.wait_connected(WAIT).await);
    assert!(oc.session.wait_online(2, WAIT).await);
    assert_eq!(c.app.status().unwrap().epoch, Some(1));

    // A가 다시 나타나 이전 채널의 Presence를 보면 질의 없이도 먼저 밀어 준다.
    let mut oa = go_online(&relay, &a);
    ob.health(GroupHealth::Removed).await;
    oc.epoch(2).await;

    assert!(oc.session.wait_online(2, WAIT).await);
    assert!(oa.session.wait_online(2, WAIT).await);
    assert_eq!(oa.session.send_text("pushed").await.unwrap(), 1);
    assert_eq!(oc.clip().await.text, "pushed");
    ob.no_clip().await;
    oa.no_clip().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn rename_reaches_online_members() {
    let relay = relay().await;
    let (a, b, c) = trio();
    let (oa, ob, oc) = (
        go_online(&relay, &a),
        go_online(&relay, &b),
        go_online(&relay, &c),
    );
    all_ready(&[&oa, &ob, &oc]).await;

    assert_eq!(oa.session.rename("laptop-2").await.unwrap(), 2);
    // 상대가 로스터를 채택할 때까지 잠깐 기다린다.
    timeout(WAIT, async {
        loop {
            let both = [&b, &c]
                .iter()
                .all(|p| p.app.devices().iter().any(|d| d.name == "laptop-2"));
            if both {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("이름이 전파되지 않음");
    relay.shutdown().await;
}
