//! 이미지 송수신 통합 테스트: 실제 릴레이와 `Session`으로 큰 이미지를 주고받는다.

use std::sync::Arc;
use std::time::Duration;

use clip_ferry_core::api::{now_ms, Clip, ClipFerry, Config, Error, ImageClip};
use clip_ferry_core::pairing::{InviterEvent, JoinerEvent};
use clip_ferry_core::session::{EventSink, SendError, Session};
use clip_ferry_core::store::MemorySecretStore;
use clip_ferry_core::transfer::MAX_IMAGE_BYTES;
use clip_ferry_core::transport::TransportConfig;
use clip_ferry_relay::{spawn, Config as RelayConfig, RelayHandle};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(20);

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
struct Collect {
    images: Option<mpsc::UnboundedSender<ImageClip>>,
    clips: Option<mpsc::UnboundedSender<Clip>>,
}

impl EventSink for Collect {
    fn on_clip(&self, clip: Clip) {
        if let Some(tx) = &self.clips {
            let _ = tx.send(clip);
        }
    }
    fn on_image(&self, image: ImageClip) {
        if let Some(tx) = &self.images {
            let _ = tx.send(image);
        }
    }
}

struct Online {
    session: Session,
    images: mpsc::UnboundedReceiver<ImageClip>,
    clips: mpsc::UnboundedReceiver<Clip>,
}

/// 릴레이는 기본 설정 그대로(초당 50프레임)라서, 큰 이미지는 속도 제한에 걸렸다가 다시 보내진다.
async fn relay() -> RelayHandle {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    spawn(listener, RelayConfig::default())
}

fn go_online(relay: &RelayHandle, p: &Profile) -> Online {
    let (itx, images) = mpsc::unbounded_channel();
    let (ctx, clips) = mpsc::unbounded_channel();
    let session = p
        .app
        .start_session(
            &format!("ws://{}", relay.local_addr()),
            TransportConfig {
                backoff_initial: Duration::from_millis(50),
                backoff_max: Duration::from_millis(200),
                ..TransportConfig::default()
            },
            Arc::new(Collect {
                images: Some(itx),
                clips: Some(ctx),
            }),
        )
        .unwrap();
    Online {
        session,
        images,
        clips,
    }
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

/// 압축이 안 되는 의사 난수 바이트.
fn noise(len: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

#[tokio::test]
async fn a_five_mb_image_reaches_both_other_devices() {
    let relay = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone", false);
    let c = profile("tablet", false);
    pair(&a, &b);
    pair(&a, &c);
    // c가 합류하면서 b의 로스터가 바뀌므로 b는 RosterUpdate를 받아야 하지만, 여기서는 세션이 처리한다.
    let (oa, mut ob, mut oc) = (
        go_online(&relay, &a),
        go_online(&relay, &b),
        go_online(&relay, &c),
    );
    ready(&[&oa, &ob, &oc]).await;

    let bytes = noise(5 * 1024 * 1024);
    let delivered = oa
        .session
        .send_image(&bytes, "image/png", 1920, 1080)
        .await
        .unwrap();
    assert_eq!(delivered, 2);
    for o in [&mut ob, &mut oc] {
        let got = timeout(WAIT, o.images.recv())
            .await
            .expect("이미지가 오지 않음")
            .unwrap();
        assert_eq!(got.bytes, bytes);
        assert_eq!(got.mime, "image/png");
        assert_eq!((got.width, got.height), (1920, 1080));
        assert_eq!(got.from_name, "laptop");
    }
    relay.shutdown().await;
}

#[tokio::test]
async fn a_twenty_mib_image_survives_the_relay_rate_limit() {
    let relay = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let (oa, mut ob) = (go_online(&relay, &a), go_online(&relay, &b));
    ready(&[&oa, &ob]).await;

    // 81건(시작 1 + 청크 80)이라 초당 50프레임 제한을 넘는다.
    let bytes = noise(MAX_IMAGE_BYTES);
    oa.session
        .send_image(&bytes, "image/webp", 4000, 3000)
        .await
        .unwrap();
    let got = timeout(WAIT, ob.images.recv())
        .await
        .expect("이미지가 오지 않음")
        .unwrap();
    assert_eq!(got.bytes.len(), MAX_IMAGE_BYTES);
    assert!(got.bytes == bytes, "내용이 달라짐");
    relay.shutdown().await;
}

#[tokio::test]
async fn oversized_or_unsupported_images_are_refused_before_sending() {
    let relay = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let (oa, mut ob) = (go_online(&relay, &a), go_online(&relay, &b));
    ready(&[&oa, &ob]).await;

    let too_big = vec![0u8; MAX_IMAGE_BYTES + 1];
    assert!(matches!(
        oa.session.send_image(&too_big, "image/png", 1, 1).await,
        Err(SendError::App(Error::ImageTooLarge))
    ));
    assert!(matches!(
        oa.session.send_image(&[1, 2, 3], "image/bmp", 1, 1).await,
        Err(SendError::App(Error::UnsupportedImage))
    ));
    assert!(timeout(Duration::from_millis(300), ob.images.recv())
        .await
        .is_err());
    relay.shutdown().await;
}

#[tokio::test]
async fn text_still_works_around_images() {
    let relay = relay().await;
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let (oa, mut ob) = (go_online(&relay, &a), go_online(&relay, &b));
    ready(&[&oa, &ob]).await;

    oa.session.send_text("before").await.unwrap();
    oa.session
        .send_image(&noise(600 * 1024), "image/jpeg", 10, 10)
        .await
        .unwrap();
    oa.session.send_text("after").await.unwrap();

    let first = timeout(WAIT, ob.clips.recv()).await.unwrap().unwrap();
    let second = timeout(WAIT, ob.clips.recv()).await.unwrap().unwrap();
    assert_eq!(
        (first.text.as_str(), second.text.as_str()),
        ("before", "after")
    );
    let img = timeout(WAIT, ob.images.recv()).await.unwrap().unwrap();
    assert_eq!(img.bytes.len(), 600 * 1024);
    relay.shutdown().await;
}
