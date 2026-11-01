//! 이미지 봉인·수신 (네트워크 없이).

use super::text_tests::{pair, profile, T0};
use super::*;
use crate::transfer::{ASSEMBLY_TIMEOUT_MS, CHUNK_BYTES, MAX_IMAGE_BYTES};

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 253) as u8).collect()
}

fn deliver(app: &ClipFerry, sealed: &SealedMessage, now: u64) -> Result<Incoming, DropReason> {
    app.handle_deliver(&sealed.bytes, Source::Relay, now)
}

fn image(incoming: Result<Incoming, DropReason>) -> ImageClip {
    match incoming {
        Ok(Incoming::Image(i)) => i,
        other => panic!("이미지가 아님: {other:?}"),
    }
}

#[test]
fn image_reaches_the_other_device_after_the_last_chunk() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let bytes = data(CHUNK_BYTES * 2 + 99);

    let sealed = a.app.seal_image(&bytes, "image/png", 30, 40, T0).unwrap();
    assert_eq!(sealed.len(), 4, "시작 1건 + 청크 3건");
    let (last, before) = sealed.split_last().unwrap();
    for m in before {
        assert_eq!(deliver(&b.app, m, T0), Ok(Incoming::Nothing));
    }
    let got = image(deliver(&b.app, last, T0));
    assert_eq!(got.bytes, bytes);
    assert_eq!(got.mime, "image/png");
    assert_eq!((got.width, got.height), (30, 40));
    assert_eq!(got.from_name, "laptop");
    assert_eq!(got.from, a.app.status().unwrap().device_id);
}

#[test]
fn chunks_may_be_delivered_out_of_order_but_not_before_the_start() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let bytes = data(CHUNK_BYTES * 3);
    let sealed = a.app.seal_image(&bytes, "image/jpeg", 1, 1, T0).unwrap();

    // 시작보다 먼저 온 청크는 버린다.
    assert_eq!(deliver(&b.app, &sealed[1], T0), Err(DropReason::BadImage));
    // 시작 뒤에는 순서가 뒤바뀌어도 된다 (첫 청크는 이미 한 번 봤으므로 새로 받는 전송을 따로 만든다).
    let again = a.app.seal_image(&bytes, "image/jpeg", 1, 1, T0).unwrap();
    assert_eq!(deliver(&b.app, &again[0], T0), Ok(Incoming::Nothing));
    assert_eq!(deliver(&b.app, &again[3], T0), Ok(Incoming::Nothing));
    assert_eq!(deliver(&b.app, &again[1], T0), Ok(Incoming::Nothing));
    assert_eq!(image(deliver(&b.app, &again[2], T0)).bytes, bytes);
    assert_eq!(b.app.drop_counts()[&DropReason::BadImage], 1);
}

#[test]
fn a_missing_chunk_never_produces_an_image_and_times_out() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let sealed = a
        .app
        .seal_image(&data(CHUNK_BYTES * 2), "image/gif", 1, 1, T0)
        .unwrap();
    // 시작과 첫 청크만 도착
    deliver(&b.app, &sealed[0], T0).unwrap();
    deliver(&b.app, &sealed[1], T0).unwrap();
    // 60초가 지나 마지막 청크가 와도 이미지는 나오지 않는다.
    let late = T0 + ASSEMBLY_TIMEOUT_MS;
    assert_eq!(deliver(&b.app, &sealed[2], late), Err(DropReason::BadImage));
}

#[test]
fn chunks_of_another_transfer_are_not_mixed_in() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let bytes = data(CHUNK_BYTES + 5);
    // 같은 크기지만 내용이 다른 이미지의 청크를 섞어 보낸다 (해시 불일치 자체는 transfer 단위 테스트).
    let good = a.app.seal_image(&bytes, "image/png", 1, 1, T0).unwrap();
    let other = a
        .app
        .seal_image(
            &data(CHUNK_BYTES + 5).iter().map(|b| !b).collect::<Vec<_>>(),
            "image/png",
            1,
            1,
            T0,
        )
        .unwrap();
    deliver(&b.app, &good[0], T0).unwrap();
    deliver(&b.app, &good[1], T0).unwrap();
    // 다른 전송의 청크는 모르는 전송이다.
    assert_eq!(deliver(&b.app, &other[2], T0), Err(DropReason::BadImage));
    assert_eq!(image(deliver(&b.app, &good[2], T0)).bytes, bytes);
}

#[test]
fn sender_side_limits() {
    let a = profile("laptop", true);
    assert!(matches!(
        a.app.seal_image(&[], "image/png", 1, 1, T0),
        Err(Error::EmptyImage)
    ));
    assert!(matches!(
        a.app.seal_image(&[1], "image/bmp", 1, 1, T0),
        Err(Error::UnsupportedImage)
    ));
    assert!(matches!(
        a.app
            .seal_image(&vec![0; MAX_IMAGE_BYTES + 1], "image/png", 1, 1, T0),
        Err(Error::ImageTooLarge)
    ));
    assert!(a
        .app
        .seal_image(&vec![0; MAX_IMAGE_BYTES], "image/png", 1, 1, T0)
        .is_ok());
}

#[test]
fn a_device_without_a_group_cannot_send_images() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    // 그룹이 없는 기기는 보낼 수 없다.
    let lone = profile("lone", false);
    assert!(matches!(
        lone.app.seal_image(&[1], "image/png", 1, 1, T0),
        Err(Error::NoGroup)
    ));
}

#[test]
fn only_two_images_per_device_are_assembled_at_once() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let imgs: Vec<_> = (0..3)
        .map(|_| {
            a.app
                .seal_image(&data(CHUNK_BYTES + 1), "image/png", 1, 1, T0)
                .unwrap()
        })
        .collect();
    deliver(&b.app, &imgs[0][0], T0).unwrap();
    deliver(&b.app, &imgs[1][0], T0).unwrap();
    assert_eq!(deliver(&b.app, &imgs[2][0], T0), Err(DropReason::BadImage));
}
