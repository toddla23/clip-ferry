use proptest::prelude::*;
use prost::Message;

use super::*;
use crate::pb::payload_body::Kind;

const NOW: u64 = 1_700_000_000_000;
const MIN: u64 = 60_000;

struct Fixture {
    group: GroupKey,
    alice: DeviceKeys,
    bob: DeviceKeys,
    alice_ctx: EpochContext,
    bob_rx: Receiver,
}

impl Fixture {
    fn new() -> Self {
        let group = GroupKey::from_bytes([0x42; 32]);
        let alice = DeviceKeys::generate();
        let bob = DeviceKeys::generate();
        let pubs = [alice.sign_public(), bob.sign_public()];
        let mut bob_rx = Receiver::new(bob.device_id());
        bob_rx.add_epoch(EpochContext::new(&group, 1, pubs));
        Self {
            alice_ctx: EpochContext::new(&group, 1, pubs),
            group,
            alice,
            bob,
            bob_rx,
        }
    }

    fn text(&self, s: &str, at: u64) -> Outgoing {
        seal_message(
            &self.alice,
            &self.alice_ctx,
            Kind::Text(pb::ClipText { text: s.into() }),
            at,
        )
        .unwrap()
    }

    /// 지정한 서명자·본문·버전으로 봉투를 직접 만든다.
    fn forged(&self, signer: &DeviceKeys, version: u32, body: &pb::PayloadBody) -> Vec<u8> {
        build(signer, &self.alice_ctx, version, body).unwrap()
    }

    fn body(&self, sender: &DeviceKeys, msg_id: u8, at: u64) -> pb::PayloadBody {
        pb::PayloadBody {
            sender_device_id: sender.device_id().0.to_vec(),
            msg_id: vec![msg_id; 16],
            created_at_ms: at,
            epoch: 1,
            kind: Some(Kind::Text(pb::ClipText { text: "x".into() })),
        }
    }

    fn recv(&mut self, bytes: &[u8]) -> Result<Received, DropReason> {
        self.bob_rx.receive(bytes, Source::Relay, NOW)
    }
}

#[test]
fn roundtrip_text() {
    let mut f = Fixture::new();
    let out = f.text("안녕", NOW);
    let got = f.recv(&out.bytes).unwrap();
    assert_eq!(got.sender, f.alice.device_id());
    assert_eq!(got.msg_id, out.msg_id);
    assert_eq!(got.epoch, 1);
    assert_eq!(got.created_at_ms, NOW);
    assert_eq!(
        got.kind,
        Kind::Text(pb::ClipText {
            text: "안녕".into()
        })
    );
}

#[test]
fn roundtrip_image_and_control() {
    let mut f = Fixture::new();
    let kinds = [
        Kind::ImageStart(pb::ImageStart {
            transfer_id: vec![1; 16],
            mime: "image/png".into(),
            total_len: 10,
            chunk_count: 1,
            sha256: vec![2; 32],
            width: 1,
            height: 1,
        }),
        Kind::ImageChunk(pb::ImageChunk {
            transfer_id: vec![1; 16],
            index: 0,
            data: vec![3; 10],
        }),
        Kind::Control(pb::Control {
            kind: Some(pb::control::Kind::Resend(pb::ResendRequest {
                msg_id: vec![9; 16],
            })),
        }),
    ];
    for kind in kinds {
        let out = seal_message(&f.alice, &f.alice_ctx, kind.clone(), NOW).unwrap();
        assert_eq!(f.recv(&out.bytes).unwrap().kind, kind);
    }
}

#[test]
fn step1_malformed_bytes() {
    let mut f = Fixture::new();
    assert_eq!(f.recv(&[0xff, 0xff, 0xff]), Err(DropReason::Malformed));
}

#[test]
fn step1_bad_nonce_length() {
    let mut f = Fixture::new();
    let mut env = pb::SealedEnvelope::decode(f.text("a", NOW).bytes.as_slice()).unwrap();
    env.nonce.pop();
    assert_eq!(f.recv(&env.encode_to_vec()), Err(DropReason::Malformed));
}

#[test]
fn step1_unsupported_version() {
    let mut f = Fixture::new();
    let bytes = f.forged(&f.alice, 2, &f.body(&f.alice, 1, NOW));
    assert_eq!(f.recv(&bytes), Err(DropReason::UnsupportedVersion));
}

#[test]
fn step1_unknown_epoch() {
    let mut f = Fixture::new();
    let ctx2 = EpochContext::new(&f.group, 2, [f.alice.sign_public()]);
    let out = seal_message(
        &f.alice,
        &ctx2,
        Kind::Text(pb::ClipText { text: "a".into() }),
        NOW,
    )
    .unwrap();
    assert_eq!(f.recv(&out.bytes), Err(DropReason::UnknownEpoch));
}

#[test]
fn previous_epoch_in_grace_is_accepted() {
    let mut f = Fixture::new();
    let pubs = [f.alice.sign_public(), f.bob.sign_public()];
    f.bob_rx.add_epoch(EpochContext::new(&f.group, 2, pubs));
    let out = f.text("old epoch", NOW); // epoch 1로 봉인
    assert!(f.recv(&out.bytes).is_ok());
    f.bob_rx.remove_epoch(1);
    let out = f.text("after removal", NOW);
    assert_eq!(f.recv(&out.bytes), Err(DropReason::UnknownEpoch));
}

#[test]
fn step2_tampered_ciphertext() {
    let mut f = Fixture::new();
    let mut env = pb::SealedEnvelope::decode(f.text("a", NOW).bytes.as_slice()).unwrap();
    env.ciphertext[0] ^= 0x01;
    assert_eq!(f.recv(&env.encode_to_vec()), Err(DropReason::Decrypt));
}

#[test]
fn step2_wrong_group_key() {
    let mut f = Fixture::new();
    let other = GroupKey::from_bytes([0x43; 32]);
    let ctx = EpochContext::new(&other, 1, [f.alice.sign_public()]);
    let out = seal_message(
        &f.alice,
        &ctx,
        Kind::Text(pb::ClipText { text: "a".into() }),
        NOW,
    )
    .unwrap();
    assert_eq!(f.recv(&out.bytes), Err(DropReason::Decrypt));
}

#[test]
fn step3_body_garbage_or_bad_lengths() {
    let mut f = Fixture::new();

    let mut body = f.body(&f.alice, 1, NOW);
    body.msg_id = vec![1; 15];
    assert_eq!(
        f.recv(&f.forged(&f.alice, 1, &body)),
        Err(DropReason::BadBody)
    );

    let mut body = f.body(&f.alice, 1, NOW);
    body.sender_device_id = vec![1; 3];
    assert_eq!(
        f.recv(&f.forged(&f.alice, 1, &body)),
        Err(DropReason::BadBody)
    );

    let mut body = f.body(&f.alice, 1, NOW);
    body.kind = None;
    assert_eq!(
        f.recv(&f.forged(&f.alice, 1, &body)),
        Err(DropReason::BadBody)
    );
}

#[test]
fn step3_epoch_mismatch() {
    let mut f = Fixture::new();
    let mut body = f.body(&f.alice, 1, NOW);
    body.epoch = 2;
    assert_eq!(
        f.recv(&f.forged(&f.alice, 1, &body)),
        Err(DropReason::EpochMismatch)
    );
}

#[test]
fn step4_sender_not_in_roster() {
    let mut f = Fixture::new();
    let carol = DeviceKeys::generate();
    let bytes = f.forged(&carol, 1, &f.body(&carol, 1, NOW));
    assert_eq!(f.recv(&bytes), Err(DropReason::UnknownSender));
}

#[test]
fn step5_forged_signature() {
    // 그룹 키를 아는 carol이 alice를 사칭: 로스터에는 있지만 서명이 맞지 않는다.
    let mut f = Fixture::new();
    let carol = DeviceKeys::generate();
    let bytes = f.forged(&carol, 1, &f.body(&f.alice, 1, NOW));
    assert_eq!(f.recv(&bytes), Err(DropReason::BadSignature));
}

#[test]
fn step6_duplicate_msg_id() {
    let mut f = Fixture::new();
    let out = f.text("a", NOW);
    assert!(f.recv(&out.bytes).is_ok());
    assert_eq!(f.recv(&out.bytes), Err(DropReason::Duplicate));
}

#[test]
fn step6_dedup_remembers_only_recent_4096() {
    let mut f = Fixture::new();
    for i in 0..=4096u32 {
        let mut id = [0u8; 16];
        id[..4].copy_from_slice(&i.to_le_bytes());
        f.bob_rx.remember(id);
    }
    let id = |i: u32| {
        let mut id = [0u8; 16];
        id[..4].copy_from_slice(&i.to_le_bytes());
        id
    };
    assert!(!f.bob_rx.has_seen(&id(0)));
    assert!(f.bob_rx.has_seen(&id(1)));
    assert!(f.bob_rx.has_seen(&id(4096)));
}

#[test]
fn step7_time_window() {
    let mut f = Fixture::new();
    let too_old = f.text("old", NOW - 11 * MIN);
    assert_eq!(f.recv(&too_old.bytes), Err(DropReason::OutsideTimeWindow));
    let too_new = f.text("new", NOW + 11 * MIN);
    assert_eq!(f.recv(&too_new.bytes), Err(DropReason::OutsideTimeWindow));
    let edge_old = f.text("edge", NOW - 10 * MIN);
    assert!(f.recv(&edge_old.bytes).is_ok());
    let edge_new = f.text("edge", NOW + 10 * MIN);
    assert!(f.recv(&edge_new.bytes).is_ok());
}

#[test]
fn step7_push_allows_older_messages() {
    let mut f = Fixture::new();
    let half_hour = f.text("late push", NOW - 30 * MIN);
    assert_eq!(f.recv(&half_hour.bytes), Err(DropReason::OutsideTimeWindow));
    assert!(f
        .bob_rx
        .receive(&half_hour.bytes, Source::Push, NOW)
        .is_ok());
    let two_hours = f.text("too late", NOW - 120 * MIN);
    assert_eq!(
        f.bob_rx.receive(&two_hours.bytes, Source::Push, NOW),
        Err(DropReason::OutsideTimeWindow)
    );
}

#[test]
fn step8_own_message_is_dropped() {
    let f = Fixture::new();
    let mut alice_rx = Receiver::new(f.alice.device_id());
    alice_rx.add_epoch(EpochContext::new(
        &f.group,
        1,
        [f.alice.sign_public(), f.bob.sign_public()],
    ));
    let out = f.text("echo", NOW);
    assert_eq!(
        alice_rx.receive(&out.bytes, Source::Relay, NOW),
        Err(DropReason::FromSelf)
    );
}

#[test]
fn drop_counters_increase() {
    let mut f = Fixture::new();
    let out = f.text("a", NOW);
    assert!(f.recv(&out.bytes).is_ok());
    let _ = f.recv(&out.bytes);
    let _ = f.recv(&out.bytes);
    let _ = f.recv(&[0xff]);
    assert_eq!(f.bob_rx.drop_counts()[&DropReason::Duplicate], 2);
    assert_eq!(f.bob_rx.drop_counts()[&DropReason::Malformed], 1);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let mut f = Fixture::new();
        prop_assert!(f.recv(&bytes).is_err());
    }

    #[test]
    fn arbitrary_envelope_fields_never_panic(
        version in 0u32..4,
        epoch in 0u64..4,
        nonce in proptest::collection::vec(any::<u8>(), 0..32),
        ciphertext in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let mut f = Fixture::new();
        let bytes = pb::SealedEnvelope { version, epoch, nonce, ciphertext }.encode_to_vec();
        prop_assert!(f.recv(&bytes).is_err());
    }

    #[test]
    fn mutated_valid_envelope_never_panics_or_passes(
        pos in 0usize..4096,
        bit in 0u8..8,
    ) {
        let mut f = Fixture::new();
        let mut bytes = f.text("hello", NOW).bytes;
        let i = pos % bytes.len();
        bytes[i] ^= 1 << bit;
        prop_assert!(f.recv(&bytes).is_err());
    }
}
