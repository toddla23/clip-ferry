//! 텍스트 봉인·수신과 로스터 채택 (네트워크 없이).

use super::*;
use crate::pairing::{InviterEvent, JoinerEvent};
use crate::store::MemorySecretStore;

pub(super) const T0: u64 = 1_700_000_000_000;

pub(super) struct Profile {
    pub(super) app: ClipFerry,
    pub(super) dir: tempfile::TempDir,
    pub(super) secrets: MemorySecretStore,
}

pub(super) fn profile(name: &str, group: bool) -> Profile {
    let dir = tempfile::tempdir().unwrap();
    let secrets = MemorySecretStore::default();
    let app = open(dir.path(), &secrets);
    app.init_device(name).unwrap();
    if group {
        app.create_group().unwrap();
    }
    Profile { app, dir, secrets }
}

pub(super) fn open(dir: &std::path::Path, secrets: &MemorySecretStore) -> ClipFerry {
    ClipFerry::open(
        Config {
            data_dir: dir.to_path_buf(),
        },
        Box::new(secrets.clone()),
    )
    .unwrap()
}

/// `joiner`를 `inviter`의 그룹에 넣고, 기존 구성원에게 갈 `RosterUpdate`를 돌려준다.
pub(super) fn pair(inviter: &Profile, joiner: &Profile) -> Vec<u8> {
    let mut inv = inviter.app.create_invite(None).unwrap();
    let mut j = joiner.app.join(inv.invite().clone()).unwrap();
    let InviterEvent::Offer { payload, .. } = inv.handle(&j.hello(), T0) else {
        panic!("Offer 없음");
    };
    assert!(matches!(j.handle(&payload), JoinerEvent::Offer { .. }));
    let approval = inviter.app.approve_join(&mut inv, T0).unwrap();
    let JoinerEvent::Joined(joined) = j.handle(&approval.welcome) else {
        panic!("참여 실패");
    };
    joiner.app.accept_join(joined).unwrap();
    approval.roster_update
}

pub(super) fn clip(incoming: Result<Incoming, DropReason>) -> Clip {
    match incoming {
        Ok(Incoming::Clip(c)) => c,
        other => panic!("텍스트가 아님: {other:?}"),
    }
}

#[test]
fn text_reaches_other_device() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);

    let sealed = a.app.seal_text("안녕 clip-ferry", T0).unwrap();
    assert_eq!(sealed.channel_id, a.app.connection().unwrap().channel_id);
    assert_eq!(sealed.channel_id, b.app.connection().unwrap().channel_id);

    let got = clip(b.app.handle_deliver(&sealed.bytes, Source::Relay, T0));
    assert_eq!(got.text, "안녕 clip-ferry");
    assert_eq!(got.from_name, "laptop");
    assert_eq!(got.from, a.app.status().unwrap().device_id);
    assert_eq!(got.created_at_ms, T0);
}

#[test]
fn own_message_and_duplicate_are_dropped() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);
    let sealed = a.app.seal_text("hi", T0).unwrap();

    assert_eq!(
        a.app.handle_deliver(&sealed.bytes, Source::Relay, T0),
        Err(DropReason::FromSelf)
    );
    assert!(b
        .app
        .handle_deliver(&sealed.bytes, Source::Relay, T0)
        .is_ok());
    assert_eq!(
        b.app.handle_deliver(&sealed.bytes, Source::Relay, T0),
        Err(DropReason::Duplicate)
    );
    assert_eq!(b.app.drop_counts()[&DropReason::Duplicate], 1);
}

#[test]
fn other_group_cannot_read_it() {
    let a = profile("laptop", true);
    let stranger = profile("stranger", true);
    let sealed = a.app.seal_text("secret", T0).unwrap();
    // 같은 에포크 번호여도 그룹 키가 달라 열 수 없다.
    assert_ne!(
        sealed.channel_id,
        stranger.app.connection().unwrap().channel_id
    );
    assert_eq!(
        stranger
            .app
            .handle_deliver(&sealed.bytes, Source::Relay, T0),
        Err(DropReason::Decrypt)
    );
}

#[test]
fn unicode_and_size_boundaries() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    pair(&a, &b);

    for text in ["한글 テスト", "😀👨‍👩‍👧‍👦🏳️‍🌈", "line1\nline2\r\n\ttab", "\u{0}nul"]
    {
        let sealed = a.app.seal_text(text, T0).unwrap();
        assert_eq!(
            clip(b.app.handle_deliver(&sealed.bytes, Source::Relay, T0)).text,
            text
        );
    }

    // 정확히 1 MiB는 되고, 1바이트 넘으면 거부
    let at_limit = "가".repeat(MAX_TEXT_BYTES / 3) + &"a".repeat(MAX_TEXT_BYTES % 3);
    assert_eq!(at_limit.len(), MAX_TEXT_BYTES);
    let sealed = a.app.seal_text(&at_limit, T0).unwrap();
    assert_eq!(
        clip(b.app.handle_deliver(&sealed.bytes, Source::Relay, T0)).text,
        at_limit
    );
    assert!(matches!(
        a.app.seal_text(&(at_limit + "a"), T0),
        Err(Error::TextTooLarge)
    ));
}

#[test]
fn empty_text_and_missing_group_are_rejected() {
    let a = profile("laptop", true);
    assert!(matches!(a.app.seal_text("", T0), Err(Error::EmptyText)));

    let no_group = profile("x", false);
    assert!(matches!(
        no_group.app.seal_text("hi", T0),
        Err(Error::NoGroup)
    ));
    let uninit = ClipFerry::open(
        Config {
            data_dir: tempfile::tempdir().unwrap().path().to_path_buf(),
        },
        Box::new(MemorySecretStore::default()),
    )
    .unwrap();
    assert!(matches!(
        uninit.seal_text("hi", T0),
        Err(Error::NotInitialized)
    ));
    assert_eq!(
        no_group.app.handle_deliver(b"junk", Source::Relay, T0),
        Ok(Incoming::Nothing)
    );
}

#[test]
fn third_device_is_known_after_roster_update() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    let c = profile("tablet", false);
    pair(&a, &b);
    let update_for_b = pair(&a, &c);

    // B의 로스터는 아직 A, B뿐이라 C의 메시지를 검증할 수 없다.
    let from_c = c.app.seal_text("from tablet", T0).unwrap();
    assert_eq!(b.app.devices().len(), 2);
    assert_eq!(
        b.app.handle_deliver(&from_c.bytes, Source::Relay, T0),
        Err(DropReason::UnknownSender)
    );

    // A가 보낸 RosterUpdate를 받으면 B가 로스터를 채택한다.
    assert_eq!(
        b.app.handle_deliver(&update_for_b, Source::Relay, T0),
        Ok(Incoming::DevicesChanged)
    );
    assert_eq!(b.app.devices().len(), 3);
    assert_eq!(b.app.status().unwrap().member_count, 3);
    let got = clip(b.app.handle_deliver(&from_c.bytes, Source::Relay, T0));
    assert_eq!(got.text, "from tablet");
    assert_eq!(got.from_name, "tablet");

    // 같은 업데이트를 또 받아도 (중복) 아무 일도 없다.
    assert_eq!(
        b.app.handle_deliver(&update_for_b, Source::Relay, T0),
        Err(DropReason::Duplicate)
    );
}

#[test]
fn adopted_roster_survives_restart() {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    let c = profile("tablet", false);
    pair(&a, &b);
    let update_for_b = pair(&a, &c);
    b.app
        .handle_deliver(&update_for_b, Source::Relay, T0)
        .unwrap();

    let reopened = open(b.dir.path(), &b.secrets);
    assert_eq!(reopened.devices().len(), 3);
    let from_c = c.app.seal_text("after restart", T0).unwrap();
    assert_eq!(
        clip(reopened.handle_deliver(&from_c.bytes, Source::Relay, T0)).text,
        "after restart"
    );
}

#[test]
fn roster_update_from_another_group_is_not_adopted() {
    // 다른 그룹의 RosterUpdate는 열리지 않으므로 채택되지 않는다.
    let a = profile("laptop", true);
    let b = profile("phone", false);
    let update = pair(&a, &b);
    let a_only = profile("solo", true);
    assert_eq!(
        a_only.app.handle_deliver(&update, Source::Relay, T0),
        Err(DropReason::Decrypt)
    );
    assert_eq!(a_only.app.devices().len(), 1);
}

#[test]
fn roster_helpers_pick_higher_version_then_hash() {
    let member = |n: u8| {
        let keys = DeviceKeys::generate();
        let mut m = group::self_member(&keys, &format!("d{n}"), T0);
        m.name = format!("d{n}");
        m
    };
    let base = pb::Roster {
        epoch: 1,
        version: 3,
        members: vec![member(1)],
    };
    let newer = pb::Roster {
        version: 4,
        ..base.clone()
    };
    assert!(roster_wins(&newer, &base));
    assert!(!roster_wins(&base, &newer));
    assert!(!roster_wins(&base, &base));

    // 같은 버전이면 해시가 큰 쪽이 이기며, 결과는 서로 반대여야 한다.
    let other = pb::Roster {
        members: vec![member(2)],
        ..base.clone()
    };
    assert_ne!(roster_wins(&base, &other), roster_wins(&other, &base));

    assert!(roster_is_consistent(&base));
    let mut broken = base.clone();
    broken.members[0].device_id[0] ^= 1;
    assert!(!roster_is_consistent(&broken));
    let mut short = base.clone();
    short.members[0].dh_pub.pop();
    assert!(!roster_is_consistent(&short));
}
