use prost::Message;

use super::*;
use crate::api::{ClipFerry, Config, Error};
use crate::crypto::{ChannelId, DeviceId, DeviceKeys, GroupKey};
use crate::envelope::{EpochContext, Receiver, Source};
use crate::store::MemorySecretStore;

const T0: u64 = 1_700_000_000_000;

fn app(name: &str, with_group: bool) -> (ClipFerry, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let app = ClipFerry::open(
        Config {
            data_dir: dir.path().to_path_buf(),
        },
        Box::new(MemorySecretStore::default()),
    )
    .unwrap();
    app.init_device(name).unwrap();
    if with_group {
        app.create_group().unwrap();
    }
    (app, dir)
}

/// 초대한 쪽이 `Offer`를 만들고 참여하는 쪽이 받기까지 진행한다.
fn offer_exchange(inviter: &mut Inviter, joiner: &mut Joiner, now: u64) -> (String, String) {
    let InviterEvent::Offer { payload, sas, .. } = inviter.handle(&joiner.hello(), now) else {
        panic!("Offer가 오지 않음");
    };
    let JoinerEvent::Offer { sas: sas_b, .. } = joiner.handle(&payload) else {
        panic!("Joiner가 Offer를 받지 못함");
    };
    (sas, sas_b)
}

#[test]
fn full_pairing_flow() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone-sim", false);

    let mut inviter = a.create_invite(Some("ws://127.0.0.1:8787".into())).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    let now = T0;
    let (sas_a, sas_b) = offer_exchange(&mut inviter, &mut joiner, now);
    assert_eq!(sas_a, sas_b);
    assert_eq!(sas_a.len(), 6);
    assert!(sas_a.bytes().all(|c| c.is_ascii_digit()));
    assert_eq!(inviter.pending().unwrap().name, "phone-sim");

    let approval = a.approve_join(&mut inviter, now).unwrap();
    assert_eq!(approval.pair_channel, inviter.invite().pair_channel());
    let JoinerEvent::Joined(joined) = joiner.handle(&approval.welcome) else {
        panic!("참여가 완료되지 않음");
    };
    let group_key = joined.group_key.clone();
    b.accept_join(joined).unwrap();

    // 두 프로필의 로스터가 같다.
    let key = |d: &crate::api::DeviceInfo| (d.device_id, d.name.clone(), d.added_at_ms);
    let mut da: Vec<_> = a.devices().iter().map(key).collect();
    let mut db: Vec<_> = b.devices().iter().map(key).collect();
    da.sort_by_key(|k| k.0 .0);
    db.sort_by_key(|k| k.0 .0);
    assert_eq!(da.len(), 2);
    assert_eq!(da, db);
    assert_eq!(a.status().unwrap().epoch, Some(1));
    assert_eq!(b.status().unwrap().epoch, Some(1));
    assert_eq!(a.status().unwrap().member_count, 2);
    assert_eq!(b.status().unwrap().member_count, 2);
    assert!(a.devices().iter().any(|d| d.is_self && d.name == "laptop"));
    assert!(b
        .devices()
        .iter()
        .any(|d| d.is_self && d.name == "phone-sim"));

    // 같은 그룹 키 → 같은 채널, 다른 태그
    let (ca, cb) = (a.connection().unwrap(), b.connection().unwrap());
    assert_eq!(ca.channel_id, cb.channel_id);
    assert_ne!(ca.device_tag, cb.device_tag);
    assert_eq!(ca.channel_id, group_key.channel_id(1));
    assert_eq!(approval.group_channel, ca.channel_id);

    let sealed = pb::SealedEnvelope::decode(approval.roster_update.as_slice()).unwrap();
    assert_eq!(sealed.epoch, 1);

    // 이미 참여한 기기는 다시 참여할 수 없다.
    assert!(matches!(
        b.join(Invite::generate(None)),
        Err(Error::GroupExists)
    ));
}

#[test]
fn roster_update_is_readable_by_new_member() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);
    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    offer_exchange(&mut inviter, &mut joiner, T0);
    let approval = a.approve_join(&mut inviter, T0).unwrap();
    let JoinerEvent::Joined(joined) = joiner.handle(&approval.welcome) else {
        panic!();
    };

    let sign_pubs: Vec<[u8; 32]> = joined
        .roster
        .members
        .iter()
        .map(|m| m.sign_pub.as_slice().try_into().unwrap())
        .collect();
    let mut rx = Receiver::new(DeviceId(
        joined
            .roster
            .members
            .iter()
            .find(|m| m.name == "phone")
            .unwrap()
            .device_id
            .as_slice()
            .try_into()
            .unwrap(),
    ));
    rx.add_epoch(EpochContext::new(
        &joined.group_key,
        joined.epoch,
        sign_pubs,
    ));
    let got = rx
        .receive(&approval.roster_update, Source::Relay, T0)
        .unwrap();
    let pb::payload_body::Kind::Control(pb::Control {
        kind: Some(pb::control::Kind::RosterUpdate(update)),
    }) = got.kind
    else {
        panic!("RosterUpdate가 아님");
    };
    assert_eq!(update.roster.unwrap().members.len(), 2);
}

#[test]
fn sas_is_deterministic_and_key_dependent() {
    let invite = Invite::from_secret([7; 32], None);
    let s1 = sas(&invite, &[1; 32], &[2; 32]);
    assert_eq!(s1, sas(&invite, &[1; 32], &[2; 32]));
    assert_eq!(s1.len(), 6);
    assert_ne!(s1, sas(&invite, &[1; 32], &[3; 32]));
    assert_ne!(
        s1,
        sas(&Invite::from_secret([8; 32], None), &[1; 32], &[2; 32])
    );
    // 순서가 중요하다 (A, B)
    assert_ne!(s1, sas(&invite, &[2; 32], &[1; 32]));
}

#[test]
fn sas_test_vector() {
    // 다른 구현과 비교하는 기준값: secret = 0x07*32, A = 0x01*32, B = 0x02*32
    let invite = Invite::from_secret([7; 32], None);
    assert_eq!(sas(&invite, &[1; 32], &[2; 32]), "646582");
}

#[test]
fn expired_invite_is_refused() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);

    // 만료 후에 도착한 Hello
    let mut inviter = a.create_invite(None).unwrap();
    let joiner = b.join(inviter.invite().clone()).unwrap();
    let late = inviter.expires_at_ms();
    assert_eq!(
        inviter.handle(&joiner.hello(), late),
        InviterEvent::Invalid(InviteProblem::Expired)
    );

    // 승인 대기 중에 만료
    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    let start = inviter.expires_at_ms() - INVITE_TTL_MS;
    offer_exchange(&mut inviter, &mut joiner, start);
    let deadline = inviter.expires_at_ms();
    let err = a.approve_join(&mut inviter, deadline).unwrap_err();
    assert!(matches!(err, Error::Pairing(PairingError::Expired)));
    assert_eq!(a.devices().len(), 1, "로스터가 바뀌면 안 된다");

    // 만료 직전에는 통과
    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    let start = inviter.expires_at_ms() - INVITE_TTL_MS;
    offer_exchange(&mut inviter, &mut joiner, start);
    let deadline = inviter.expires_at_ms();
    assert!(a.approve_join(&mut inviter, deadline - 1).is_ok());
}

#[test]
fn expire_sends_reject_to_joiner() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);
    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    assert!(inviter.expire(inviter.expires_at_ms() - 1).is_none());
    let payload = inviter.expire(inviter.expires_at_ms()).unwrap();
    assert!(matches!(joiner.handle(&payload), JoinerEvent::Rejected));
}

#[test]
fn used_invite_cannot_be_used_again() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);
    let (c, _dc) = app("tablet", false);

    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    offer_exchange(&mut inviter, &mut joiner, T0);
    a.approve_join(&mut inviter, T0).unwrap();

    let other = c.join(inviter.invite().clone()).unwrap();
    assert_eq!(
        inviter.handle(&other.hello(), T0 + 1),
        InviterEvent::Invalid(InviteProblem::AlreadyUsed)
    );
    assert!(matches!(
        a.approve_join(&mut inviter, T0 + 1),
        Err(Error::Pairing(
            PairingError::NotPending | PairingError::Closed
        ))
    ));
    assert_eq!(a.devices().len(), 2);
}

#[test]
fn second_joiner_invalidates_invite() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);
    let (c, _dc) = app("intruder", false);

    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    offer_exchange(&mut inviter, &mut joiner, T0);

    let intruder = c.join(inviter.invite().clone()).unwrap();
    assert_eq!(
        inviter.handle(&intruder.hello(), T0 + 1),
        InviterEvent::Invalid(InviteProblem::SecondJoiner)
    );
    // 이미 경고했으므로 이후에는 조용히 무시한다.
    assert_eq!(
        inviter.handle(&intruder.hello(), T0 + 2),
        InviterEvent::None
    );
    // 첫 참여자도 더 이상 승인할 수 없다.
    assert!(matches!(
        a.approve_join(&mut inviter, T0 + 3),
        Err(Error::Pairing(
            PairingError::NotPending | PairingError::Closed
        ))
    ));
    assert_eq!(a.devices().len(), 1);
}

#[test]
fn repeated_hello_from_same_device_is_a_retransmission() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);
    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();

    let (sas1, _) = offer_exchange(&mut inviter, &mut joiner, T0);
    // 같은 기기가 다시 보냄 → 같은 SAS로 다시 응답
    let InviterEvent::Offer { sas: sas2, .. } = inviter.handle(&joiner.hello(), T0 + 1) else {
        panic!("재전송이 무시됨");
    };
    assert_eq!(sas1, sas2);
    // 참여자는 두 번째 Offer로 상태를 바꾸지 않는다.
    let InviterEvent::Offer { payload, .. } = inviter.handle(&joiner.hello(), T0 + 2) else {
        panic!();
    };
    assert!(matches!(joiner.handle(&payload), JoinerEvent::None));
    assert!(a.approve_join(&mut inviter, T0 + 3).is_ok());
}

#[test]
fn rejection_reaches_joiner_and_closes_invite() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);
    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    offer_exchange(&mut inviter, &mut joiner, T0);

    let reject = inviter.reject().unwrap();
    assert!(matches!(joiner.handle(&reject), JoinerEvent::Rejected));
    assert!(matches!(
        a.approve_join(&mut inviter, T0 + 1),
        Err(Error::Pairing(
            PairingError::NotPending | PairingError::Closed
        ))
    ));
    assert!(inviter.reject().is_err());
    assert_eq!(a.devices().len(), 1);
}

#[test]
fn approve_without_request_fails() {
    let (a, _da) = app("laptop", true);
    let mut inviter = a.create_invite(None).unwrap();
    assert!(matches!(
        a.approve_join(&mut inviter, T0),
        Err(Error::Pairing(PairingError::NotPending))
    ));
}

#[test]
fn invite_requires_group_and_join_requires_none() {
    let (no_group, _d) = app("x", false);
    assert!(matches!(no_group.create_invite(None), Err(Error::NoGroup)));
    let (with_group, _d2) = app("y", true);
    assert!(matches!(
        with_group.join(Invite::generate(None)),
        Err(Error::GroupExists)
    ));
}

#[test]
fn messages_from_another_invite_are_ignored() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);
    let mut inviter = a.create_invite(None).unwrap();
    let other = b.join(Invite::generate(None)).unwrap();
    assert_eq!(inviter.handle(&other.hello(), T0), InviterEvent::None);
    assert_eq!(inviter.handle(b"garbage", T0), InviterEvent::None);
    assert_eq!(inviter.handle(&[], T0), InviterEvent::None);
}

// --- 환영 메시지 검증 -------------------------------------------------------

struct Raw {
    ka: DeviceKeys,
    kb: DeviceKeys,
    invite: Invite,
    joiner: Joiner,
    info: JoinerInfo,
    group: GroupKey,
}

fn raw() -> Raw {
    let ka = DeviceKeys::generate();
    let kb = DeviceKeys::generate();
    let invite = Invite::generate(None);
    let mut inviter = Inviter::new(invite.clone(), Identity::of(&ka, "a"), T0);
    let mut joiner = Joiner::new(
        invite.clone(),
        DeviceKeys::from_bytes(&kb.to_bytes()[..]).unwrap(),
        "b",
    );
    offer_exchange(&mut inviter, &mut joiner, T0);
    let info = inviter.pending().unwrap().clone();
    Raw {
        ka,
        kb,
        invite,
        joiner,
        info,
        group: GroupKey::from_bytes([9; 32]),
    }
}

fn roster_of(r: &Raw, include_b: bool, epoch: u64) -> pb::Roster {
    let mut members = vec![crate::group::self_member(&r.ka, "a", T0)];
    if include_b {
        members.push(crate::group::self_member(&r.kb, "b", T0));
    }
    pb::Roster {
        epoch,
        version: 2,
        members,
    }
}

fn deliver(r: &mut Raw, welcome: pb::PairWelcome) -> JoinerEvent {
    let payload = seal_message(&r.invite, pb::pair_message::Kind::Welcome(welcome));
    r.joiner.handle(&payload)
}

#[test]
fn raw_welcome_is_accepted() {
    let mut r = raw();
    let w = build_welcome(&r.ka, &r.info, &r.group, &roster_of(&r, true, 1)).unwrap();
    let JoinerEvent::Joined(j) = deliver(&mut r, w) else {
        panic!("받아들여지지 않음");
    };
    assert_eq!(j.epoch, 1);
    assert_eq!(j.group_key.as_bytes(), &[9; 32]);
    assert_eq!(j.roster.members.len(), 2);
}

#[test]
fn welcome_signed_by_someone_else_is_rejected() {
    let mut r = raw();
    let carol = DeviceKeys::generate();
    let w = build_welcome(&carol, &r.info, &r.group, &roster_of(&r, true, 1)).unwrap();
    assert!(matches!(
        deliver(&mut r, w),
        JoinerEvent::Failed(WelcomeError::BadSignature)
    ));
}

#[test]
fn tampered_welcome_is_rejected() {
    let mut r = raw();
    let mut w = build_welcome(&r.ka, &r.info, &r.group, &roster_of(&r, true, 1)).unwrap();
    w.wrapped.as_mut().unwrap().ct[0] ^= 1;
    assert!(matches!(
        deliver(&mut r, w),
        JoinerEvent::Failed(WelcomeError::BadSignature)
    ));

    // 서명을 다시 해도 암호문이 깨졌으면 열리지 않는다.
    let mut r = raw();
    let mut w = build_welcome(&r.ka, &r.info, &r.group, &roster_of(&r, true, 1)).unwrap();
    let key = w.wrapped.as_mut().unwrap();
    key.ct[0] ^= 1;
    let mut m = b"clip-ferry v1 welcome".to_vec();
    m.extend_from_slice(&key.eph_pub);
    m.extend_from_slice(&key.nonce);
    m.extend_from_slice(&key.ct);
    w.signature = r.ka.sign(&m).to_vec();
    assert!(matches!(
        deliver(&mut r, w),
        JoinerEvent::Failed(WelcomeError::Undecryptable)
    ));
}

#[test]
fn welcome_for_another_device_is_rejected() {
    let mut r = raw();
    let other = DeviceKeys::generate();
    let other_info = JoinerInfo {
        name: "other".into(),
        sign_pub: other.sign_public(),
        dh_pub: other.dh_public(),
    };
    let w = build_welcome(&r.ka, &other_info, &r.group, &roster_of(&r, true, 1)).unwrap();
    assert!(matches!(deliver(&mut r, w), JoinerEvent::Failed(_)));
}

#[test]
fn welcome_with_bad_roster_is_rejected() {
    // 내가 로스터에 없음
    let mut r = raw();
    let w = build_welcome(&r.ka, &r.info, &r.group, &roster_of(&r, false, 1)).unwrap();
    assert!(matches!(
        deliver(&mut r, w),
        JoinerEvent::Failed(WelcomeError::BadContents)
    ));

    // 초대한 기기가 로스터에 없음
    let mut r = raw();
    let mut roster = roster_of(&r, true, 1);
    roster.members.remove(0);
    let w = build_welcome(&r.ka, &r.info, &r.group, &roster).unwrap();
    assert!(matches!(
        deliver(&mut r, w),
        JoinerEvent::Failed(WelcomeError::BadContents)
    ));

    // 에포크 0
    let mut r = raw();
    let w = build_welcome(&r.ka, &r.info, &r.group, &roster_of(&r, true, 0)).unwrap();
    assert!(matches!(
        deliver(&mut r, w),
        JoinerEvent::Failed(WelcomeError::BadContents)
    ));
}

#[test]
fn welcome_before_offer_is_ignored() {
    let ka = DeviceKeys::generate();
    let kb = DeviceKeys::generate();
    let invite = Invite::generate(None);
    let mut joiner = Joiner::new(invite.clone(), kb, "b");
    let info = JoinerInfo {
        name: "b".into(),
        sign_pub: joiner.me.sign_pub,
        dh_pub: joiner.me.dh_pub,
    };
    let roster = pb::Roster {
        epoch: 1,
        version: 1,
        members: vec![
            crate::group::self_member(&ka, "a", T0),
            pb::Member {
                device_id: info.device_id().0.to_vec(),
                sign_pub: info.sign_pub.to_vec(),
                dh_pub: info.dh_pub.to_vec(),
                name: "b".into(),
                added_at_ms: T0,
                push: None,
            },
        ],
    };
    let w = build_welcome(&ka, &info, &GroupKey::from_bytes([1; 32]), &roster).unwrap();
    let payload = seal_message(&invite, pb::pair_message::Kind::Welcome(w));
    assert!(matches!(joiner.handle(&payload), JoinerEvent::None));
}

#[test]
fn random_bytes_never_panic() {
    let (a, _da) = app("laptop", true);
    let (b, _db) = app("phone", false);
    let mut inviter = a.create_invite(None).unwrap();
    let mut joiner = b.join(inviter.invite().clone()).unwrap();
    for len in 0..200usize {
        let junk: Vec<u8> = (0..len).map(|i| (i * 31 + len) as u8).collect();
        assert_eq!(inviter.handle(&junk, T0), InviterEvent::None);
        assert!(matches!(joiner.handle(&junk), JoinerEvent::None));
    }
}

#[test]
fn channel_id_type_is_opaque() {
    // 페어링 채널은 그룹 채널과 다른 값이다.
    let (a, _da) = app("laptop", true);
    let inviter = a.create_invite(None).unwrap();
    let group: ChannelId = a.connection().unwrap().channel_id;
    assert_ne!(inviter.invite().pair_channel(), group.0);
}
