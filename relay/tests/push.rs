//! 푸시 게이트웨이와 재전송 통합 테스트 (F22·F25): 실제 릴레이 + 가짜 푸시 서버.
//!
//! 아이폰(b)은 푸시 토큰을 로스터에 적고 오프라인이 된다. 노트북(a)이 보내면 릴레이가 푸시를 넘기고,
//! b는 푸시 페이로드를 복호화한다. 이미지는 도착 알림만 받고, 다시 붙어서 a에게 재전송을 요청한다.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clip_ferry_core::api::{now_ms, Clip, ClipFerry, Config, ImageClip, Incoming};
use clip_ferry_core::envelope::Source;
use clip_ferry_core::pairing::{InviterEvent, JoinerEvent};
use clip_ferry_core::pb;
use clip_ferry_core::session::{EventSink, Session};
use clip_ferry_core::store::MemorySecretStore;
use clip_ferry_core::transport::TransportConfig;
use clip_ferry_relay::push::{Notice, PushSender};
use clip_ferry_relay::{spawn_with_push, Config as RelayConfig, RelayHandle};
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

fn pair(inviter: &Profile, joiner: &Profile) {
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
}

#[derive(Default)]
struct FakePush(Mutex<Vec<Notice>>);

impl PushSender for FakePush {
    fn send(&self, notice: Notice) {
        self.0.lock().unwrap().push(notice);
    }
}

impl FakePush {
    async fn wait(&self, count: usize) -> Vec<Notice> {
        let end = tokio::time::Instant::now() + WAIT;
        loop {
            let got = self.0.lock().unwrap().clone();
            if got.len() >= count || tokio::time::Instant::now() > end {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

enum Got {
    Text,
    Image(ImageClip),
}

struct Collect(mpsc::UnboundedSender<Got>);

impl EventSink for Collect {
    fn on_clip(&self, _clip: Clip) {
        let _ = self.0.send(Got::Text);
    }
    fn on_image(&self, image: ImageClip) {
        let _ = self.0.send(Got::Image(image));
    }
}

fn fast() -> TransportConfig {
    TransportConfig {
        backoff_initial: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        ..TransportConfig::default()
    }
}

async fn relay(push: Arc<FakePush>) -> RelayHandle {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    spawn_with_push(
        listener,
        RelayConfig {
            max_frames_per_sec: 1000,
            ..RelayConfig::default()
        },
        push,
    )
}

fn online(relay: &RelayHandle, p: &Profile) -> (Session, mpsc::UnboundedReceiver<Got>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let session = p
        .app
        .start_session(
            &format!("ws://{}", relay.local_addr()),
            fast(),
            Arc::new(Collect(tx)),
        )
        .unwrap();
    (session, rx)
}

/// PNG 머리말이 있는 바이트 (코어는 디코딩하지 않는다).
fn png(len: usize) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend((0..len).map(|i| (i % 251) as u8));
    v
}

#[tokio::test]
async fn offline_iphone_gets_text_by_push_and_image_by_resend() {
    let push = Arc::new(FakePush::default());
    let relay = relay(push.clone()).await;
    let a = profile("laptop", true);
    let b = profile("iphone", false);
    pair(&a, &b);

    // b가 토큰을 적고 a에게 알린 뒤 오프라인이 된다.
    let (sa, mut ra) = online(&relay, &a);
    {
        let (sb, _rb) = online(&relay, &b);
        assert!(sa.wait_online(2, WAIT).await && sb.wait_online(2, WAIT).await);
        assert_eq!(
            sb.set_push_token(pb::Platform::Apns, "00ff").await.unwrap(),
            1
        );
        let end = tokio::time::Instant::now() + WAIT;
        while a.app.push_targets().is_empty() {
            assert!(
                tokio::time::Instant::now() < end,
                "토큰이 로스터에 안 들어옴"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // b가 온라인이면 푸시하지 않는다.
        assert_eq!(sa.send_text("온라인일 때").await.unwrap(), 1);
        assert!(push.0.lock().unwrap().is_empty());
    }
    let end = tokio::time::Instant::now() + WAIT;
    // b가 끊긴 것이 반영될 때까지 기다린다(전달 0).
    loop {
        if sa.send_text("확인").await.unwrap() == 0 {
            break;
        }
        assert!(tokio::time::Instant::now() < end, "b가 끊기지 않음");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    push.0.lock().unwrap().clear();

    // 텍스트: 봉투가 푸시에 담기고 b가 바로 연다.
    assert_eq!(sa.send_text("푸시로 가는 글").await.unwrap(), 0);
    let notices = push.wait(1).await;
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].token, "00ff");
    let a_tag = a
        .app
        .device_tag_of(&a.app.status().unwrap().device_id)
        .unwrap();
    assert_eq!(notices[0].from_tag, a_tag.0);
    assert_eq!(
        b.app.device_for_tag(&notices[0].from_tag).unwrap().name,
        "laptop"
    );
    let payload = notices[0].payload.clone().expect("작은 글은 봉투가 담긴다");
    let Ok(Incoming::Clip(clip)) = b.app.handle_deliver(&payload, Source::Push, now_ms()) else {
        panic!("푸시 봉투를 열지 못함");
    };
    assert_eq!(clip.text, "푸시로 가는 글");
    assert_eq!(clip.from_name, "laptop");

    // 긴 텍스트: 봉투 없이 도착 알림만.
    push.0.lock().unwrap().clear();
    sa.send_text(&"가".repeat(2000)).await.unwrap();
    let notices = push.wait(1).await;
    assert!(notices[0].payload.is_none());

    // 이미지: 시작만 푸시된다(청크는 푸시 없음).
    push.0.lock().unwrap().clear();
    let image = png(600_000);
    sa.send_image(&image, "image/png", 10, 10).await.unwrap();
    let notices = push.wait(1).await;
    assert_eq!(notices.len(), 1, "이미지는 시작에만 푸시");
    let Ok(Incoming::ImageAnnounced { from, msg_id, .. }) =
        b.app
            .handle_deliver(notices[0].payload.as_ref().unwrap(), Source::Push, now_ms())
    else {
        panic!("이미지 도착 알림이 아님");
    };

    // b가 앱을 열어 다시 붙고, a에게만 재전송을 요청한다.
    let (sb, mut rb) = online(&relay, &b);
    assert!(sb.wait_online(2, WAIT).await);
    assert_eq!(sb.request_resend(&from, &msg_id).await.unwrap(), 1);
    let got = timeout(WAIT, async {
        loop {
            if let Some(Got::Image(i)) = rb.recv().await {
                return i;
            }
        }
    })
    .await
    .expect("다시 보낸 이미지가 안 옴");
    assert_eq!(got.bytes, image);
    // 재전송은 요청한 기기에게만 가므로 a 자신은 아무것도 받지 않는다.
    assert!(timeout(Duration::from_millis(200), ra.recv())
        .await
        .is_err());

    // 모르는 msg_id는 다시 보내지 않는다.
    assert_eq!(sb.request_resend(&from, &[7; 16]).await.unwrap(), 1);
    assert!(timeout(Duration::from_millis(500), rb.recv())
        .await
        .is_err());

    // 보낸 기기가 오프라인이면 요청이 전달되지 않는다(0) — "보낸 기기가 오프라인입니다".
    drop(sa);
    let end = tokio::time::Instant::now() + WAIT;
    loop {
        if sb.request_resend(&from, &[]).await.unwrap() == 0 {
            break;
        }
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
