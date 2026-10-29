//! 기기 제거·키 회전·복구·유예·로스터 수렴 (네트워크 없이 메시지를 직접 전달).

use super::epochs::{member_by_id, wrap_key_for, GRACE_MS};
use super::text_tests::{clip, open, pair, profile, Profile, T0};
use super::*;
use crate::envelope::seal_message;

fn deliver(p: &Profile, bytes: &[u8], now: u64) -> Result<Incoming, DropReason> {
    p.app.handle_deliver(bytes, Source::Relay, now)
}

/// 세 기기가 같은 그룹에 있고 서로의 로스터를 안다.
fn trio() -> (Profile, Profile, Profile) {
    let a = profile("laptop", true);
    let b = profile("phone", false);
    let c = profile("tablet", false);
    pair(&a, &b);
    let update_for_b = pair(&a, &c);
    assert_eq!(deliver(&b, &update_for_b, T0), Ok(Incoming::DevicesChanged));
    (a, b, c)
}

fn last_contact(p: &Profile) -> u64 {
    p.app
        .device
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .last_contact_ms
}

#[test]
fn removed_device_cannot_read_new_messages() {
    let (a, b, c) = trio();
    let msgs = a.app.remove_device("phone", T0 + 1).unwrap();
    assert_eq!(msgs.len(), 1);
    let old_channel = msgs[0].channel_id.clone();

    // 남은 기기는 새 에포크로 넘어가고, 제거된 기기는 자기가 빠진 것을 안다.
    assert_eq!(
        deliver(&c, &msgs[0].bytes, T0 + 1),
        Ok(Incoming::EpochChanged(2))
    );
    assert_eq!(deliver(&b, &msgs[0].bytes, T0 + 1), Ok(Incoming::Removed));
    assert_eq!(b.app.group_health(T0), GroupHealth::Removed);
    assert!(matches!(b.app.seal_text("hi", T0 + 2), Err(Error::Removed)));

    for p in [&a, &c] {
        let st = p.app.status().unwrap();
        assert_eq!(st.epoch, Some(2));
        assert_eq!(st.member_count, 2);
        // 새 채널은 이전과 다르다.
        assert_ne!(p.app.connection().unwrap().channel_id, old_channel);
    }
    assert!(a.app.devices().iter().all(|d| d.name != "phone"));

    // 새 메시지는 C만 받고 B는 받지 못한다.
    let sealed = a.app.seal_text("after remove", T0 + 2).unwrap();
    assert_eq!(
        clip(deliver(&c, &sealed.bytes, T0 + 2)).text,
        "after remove"
    );
    assert_eq!(deliver(&b, &sealed.bytes, T0 + 2), Ok(Incoming::Nothing));
}

#[test]
fn removed_device_cannot_decrypt_even_with_old_state() {
    let (a, b, c) = trio();
    // 제거 소식을 받지 못한 B (옛 상태 그대로)
    let b_old = open(b.dir.path(), &b.secrets);
    let msgs = a.app.remove_device("phone", T0 + 1).unwrap();
    deliver(&c, &msgs[0].bytes, T0 + 1).unwrap();

    let sealed = a.app.seal_text("secret after remove", T0 + 2).unwrap();
    assert_eq!(
        b_old.handle_deliver(&sealed.bytes, Source::Relay, T0 + 2),
        Err(DropReason::UnknownEpoch)
    );
}

#[test]
fn removed_device_cannot_inject_via_old_epoch() {
    let (a, b, c) = trio();
    let b_old = open(b.dir.path(), &b.secrets);
    let msgs = a.app.remove_device("phone", T0 + 1).unwrap();
    deliver(&c, &msgs[0].bytes, T0 + 1).unwrap();

    // 옛 키로 옛 채널에 보낸다: 유예 중이라 봉투는 열리지만 보낸 기기가 지금 그룹에 없으므로 버린다.
    let evil = b_old.seal_text("evil", T0 + 3).unwrap();
    assert_eq!(evil.channel_id, msgs[0].channel_id);
    assert_eq!(
        deliver(&c, &evil.bytes, T0 + 3),
        Err(DropReason::SenderRemoved)
    );
    assert_eq!(
        deliver(&a, &evil.bytes, T0 + 3),
        Err(DropReason::SenderRemoved)
    );
    assert_eq!(c.app.drop_counts()[&DropReason::SenderRemoved], 1);
}

#[test]
fn offline_member_catches_up_with_epoch_query() {
    let (a, _b, c) = trio();
    let _ = a.app.remove_device("phone", T0 + 1).unwrap();
    // C는 그동안 꺼져 있어 Rekey를 못 받았다. 돌아와서 묻는다.
    let query = c.app.seal_epoch_query(T0 + 5).unwrap();
    assert_eq!(c.app.status().unwrap().epoch, Some(1));

    let Ok(Incoming::Replies(replies)) = deliver(&a, &query.bytes, T0 + 5) else {
        panic!("응답이 없음");
    };
    assert_eq!(replies.len(), 1);
    // 응답은 C가 듣고 있는 옛 채널로 간다.
    assert_eq!(replies[0].channel_id, query.channel_id);

    assert_eq!(
        deliver(&c, &replies[0].bytes, T0 + 5),
        Ok(Incoming::EpochChanged(2))
    );
    assert_eq!(c.app.status().unwrap().epoch, Some(2));
    assert_eq!(c.app.devices().len(), 2);
    // 같은 응답이 또 와도(여러 기기가 응답) 무해하다.
    assert_eq!(
        deliver(&c, &replies[0].bytes, T0 + 5),
        Err(DropReason::Duplicate)
    );

    let sealed = a.app.seal_text("hello c", T0 + 6).unwrap();
    assert_eq!(clip(deliver(&c, &sealed.bytes, T0 + 6)).text, "hello c");

    // C는 새 채널과 유예 중인 옛 채널을 모두 구독한다.
    let subs = c.app.subscriptions();
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0].epoch, 2);
    assert_eq!(subs[1].epoch, 1);
}

#[test]
fn removed_device_learns_it_via_epoch_query() {
    let (a, b, _c) = trio();
    let _ = a.app.remove_device("phone", T0 + 1).unwrap();
    // B는 꺼져 있었다. 돌아와서 묻는다 → 로스터 없이 "제거됨" 알림만 받는다.
    let query = b.app.seal_epoch_query(T0 + 5).unwrap();
    let Ok(Incoming::Replies(replies)) = deliver(&a, &query.bytes, T0 + 5) else {
        panic!("응답이 없음");
    };
    assert_eq!(
        deliver(&b, &replies[0].bytes, T0 + 5),
        Ok(Incoming::Removed)
    );
    assert_eq!(b.app.group_health(T0 + 5), GroupHealth::Removed);
    assert!(b.app.subscriptions().is_empty());
    assert!(b.app.connection().is_none());
    assert!(matches!(
        b.app.seal_epoch_query(T0 + 6),
        Err(Error::Removed)
    ));

    // 다시 열어도 제거된 상태다.
    let reopened = open(b.dir.path(), &b.secrets);
    assert_eq!(reopened.group_health(T0 + 5), GroupHealth::Removed);
}

#[test]
fn removal_notice_does_not_leak_the_new_roster() {
    let (a, b, _c) = trio();
    let _ = a.app.remove_device("phone", T0 + 1).unwrap();
    let query = b.app.seal_epoch_query(T0 + 5).unwrap();
    let Ok(Incoming::Replies(replies)) = deliver(&a, &query.bytes, T0 + 5) else {
        panic!();
    };
    // B가 풀 수 있는 옛 키로 응답을 열어, 로스터와 키가 비어 있음을 확인한다.
    let mut rx = {
        let slot = b.app.device.lock().unwrap();
        let dev = slot.as_ref().unwrap();
        let mut rx = Receiver::new(dev.keys.device_id());
        rx.add_epoch(dev.send_context().unwrap());
        rx
    };
    let got = rx
        .receive(&replies[0].bytes, Source::Relay, T0 + 5)
        .unwrap();
    let pb::payload_body::Kind::Control(pb::Control {
        kind: Some(pb::control::Kind::Rekey(rekey)),
    }) = got.kind
    else {
        panic!("Rekey가 아님");
    };
    assert!(rekey.new_roster.is_none());
    // 알림은 B의 id만 가리키고 키 재료는 없다.
    assert_eq!(rekey.keys.len(), 1);
    assert_eq!(rekey.keys[0].device_id, b.app.status().unwrap().device_id.0);
    assert!(rekey.keys[0].ct.is_empty());
}

#[test]
fn removal_notice_for_one_device_does_not_affect_another() {
    let (a, b, c) = trio();
    let _ = a.app.remove_device("phone", T0 + 1).unwrap();
    // B가 묻고, A가 B에게 "제거됨" 알림을 보낸다. 같은 옛 채널에 있는 C도 이 알림을 듣는다.
    let query = b.app.seal_epoch_query(T0 + 5).unwrap();
    let Ok(Incoming::Replies(replies)) = deliver(&a, &query.bytes, T0 + 5) else {
        panic!();
    };
    assert_eq!(
        deliver(&c, &replies[0].bytes, T0 + 5),
        Ok(Incoming::Nothing)
    );
    assert_eq!(c.app.group_health(T0 + 5), GroupHealth::Active);
    assert_eq!(c.app.status().unwrap().epoch, Some(1));
    // 여전히 C는 따라잡을 수 있다.
    let query = c.app.seal_epoch_query(T0 + 6).unwrap();
    let Ok(Incoming::Replies(replies)) = deliver(&a, &query.bytes, T0 + 6) else {
        panic!();
    };
    assert_eq!(
        deliver(&c, &replies[0].bytes, T0 + 6),
        Ok(Incoming::EpochChanged(2))
    );
}

#[test]
fn present_devices_on_the_old_channel_are_pushed_the_new_key() {
    let (a, b, c) = trio();
    let _ = a.app.remove_device("phone", T0 + 1).unwrap();

    let tag = |p: &Profile, i: usize| p.app.subscriptions()[i].device_tag.0.to_vec();
    // 옛 채널(에포크 1)에 B, C와 A 자신, 모르는 기기가 붙어 있다.
    let online = vec![tag(&b, 0), tag(&c, 0), tag(&a, 1), vec![9; 16]];
    let replies = a.app.catch_up_replies(1, &online, T0 + 5);
    assert_eq!(
        replies.len(),
        2,
        "B와 C에게만 (자기 자신과 모르는 태그는 제외)"
    );
    let old_channel = c.app.connection().unwrap().channel_id;
    assert!(replies.iter().all(|m| m.channel_id == old_channel));

    // 같은 채널이라 모두가 모든 응답을 듣는다. 각자 자기 몫만 처리한다.
    for m in &replies {
        let _ = deliver(&b, &m.bytes, T0 + 5);
        let _ = deliver(&c, &m.bytes, T0 + 5);
    }
    assert_eq!(b.app.group_health(T0 + 5), GroupHealth::Removed);
    assert_eq!(c.app.status().unwrap().epoch, Some(2));
    assert_eq!(c.app.group_health(T0 + 5), GroupHealth::Active);

    // 현재 또는 미래 에포크, 아무도 없는 채널에는 보내지 않는다.
    assert!(a.app.catch_up_replies(2, &online, T0 + 5).is_empty());
    assert!(a.app.catch_up_replies(1, &[], T0 + 5).is_empty());
    assert!(a.app.catch_up_replies(9, &online, T0 + 5).is_empty());
}

#[test]
fn grace_period_expires_after_thirty_days() {
    let (a, _b, c) = trio();
    let removed_at = T0 + 1;
    let _ = a.app.remove_device("phone", removed_at).unwrap();
    assert_eq!(a.app.subscriptions().len(), 2);

    // 정확히 30일째까지는 유지
    assert!(!a.app.prune_expired(removed_at + GRACE_MS));
    assert!(a.secrets.get("group_key/1").is_some());
    let query = c.app.seal_epoch_query(removed_at + 10).unwrap();
    assert!(matches!(
        deliver(&a, &query.bytes, removed_at + 10),
        Ok(Incoming::Replies(_))
    ));

    // 30일이 넘으면 이전 에포크의 키와 구독을 지운다.
    assert!(a.app.prune_expired(removed_at + GRACE_MS + 1));
    assert!(a.secrets.get("group_key/1").is_none());
    assert!(a.secrets.get("group_key/2").is_some());
    assert_eq!(a.app.subscriptions().len(), 1);
    let late = c.app.seal_epoch_query(removed_at + GRACE_MS + 2).unwrap();
    assert_eq!(
        deliver(&a, &late.bytes, removed_at + GRACE_MS + 2),
        Err(DropReason::UnknownEpoch)
    );

    // 다시 열어도 지워진 채다.
    let reopened = open(a.dir.path(), &a.secrets);
    assert_eq!(reopened.subscriptions().len(), 1);
    assert!(!reopened.prune_expired(removed_at + GRACE_MS + 3));
}

#[test]
fn stale_after_thirty_days_without_contact() {
    let (a, _b, c) = trio();
    let base = last_contact(&c);

    assert_eq!(c.app.group_health(base), GroupHealth::Active);
    assert_eq!(c.app.group_health(base + GRACE_MS), GroupHealth::Active);
    assert_eq!(c.app.group_health(base + GRACE_MS + 1), GroupHealth::Stale);

    // 다른 기기의 유효한 메시지를 받으면 다시 정상이다.
    let later = base + GRACE_MS + 10;
    let sealed = a.app.seal_text("still here", later).unwrap();
    assert_eq!(
        clip(c.app.handle_deliver(&sealed.bytes, Source::Relay, later)).text,
        "still here"
    );
    assert_eq!(c.app.group_health(later + 1), GroupHealth::Active);
    assert_eq!(c.app.group_health(later + GRACE_MS + 1), GroupHealth::Stale);
}

#[test]
fn single_device_group_is_never_stale() {
    let solo = profile("solo", true);
    let base = last_contact(&solo);
    assert_eq!(
        solo.app.group_health(base + 10 * GRACE_MS),
        GroupHealth::Active
    );
}

#[test]
fn stale_device_recovers_when_it_gets_a_newer_epoch() {
    let (a, _b, c) = trio();
    let _ = a.app.remove_device("phone", T0 + 1).unwrap();
    let base = last_contact(&c);
    let far = base + GRACE_MS + 100;
    assert_eq!(c.app.group_health(far), GroupHealth::Stale);
    // 오래 뒤에도 Rekey를 받으면 따라잡는다(발신 시각이 창 안이어야 하므로 지금 시각으로 만든 응답).
    let query = c.app.seal_epoch_query(far).unwrap();
    // 보낸 기기의 이전 에포크 키는 아직 정리되기 전이므로 응답이 온다.
    let Ok(Incoming::Replies(replies)) = deliver(&a, &query.bytes, far) else {
        panic!("응답이 없음");
    };
    deliver(&c, &replies[0].bytes, far).unwrap();
    assert_eq!(c.app.group_health(far + 1), GroupHealth::Active);
}

fn names(p: &Profile) -> Vec<String> {
    let mut n: Vec<_> = p.app.devices().into_iter().map(|d| d.name).collect();
    n.sort();
    n
}

/// A와 B가 동시에 이름을 바꾸고, C는 두 업데이트를 서로 다른 순서로 받는다.
fn concurrent_renames(c_gets_a_first: bool) -> [Vec<String>; 3] {
    let (a, b, c) = trio();
    let ua = a.app.rename("a-renamed", T0 + 10).unwrap();
    let ub = b.app.rename("b-renamed", T0 + 10).unwrap();

    let _ = deliver(&a, &ub.bytes, T0 + 11);
    let _ = deliver(&b, &ua.bytes, T0 + 11);
    if c_gets_a_first {
        let _ = deliver(&c, &ua.bytes, T0 + 11);
        let _ = deliver(&c, &ub.bytes, T0 + 11);
    } else {
        let _ = deliver(&c, &ub.bytes, T0 + 11);
        let _ = deliver(&c, &ua.bytes, T0 + 11);
    }
    [names(&a), names(&b), names(&c)]
}

#[test]
fn concurrent_roster_updates_converge_on_every_device() {
    for c_first in [true, false] {
        let [na, nb, nc] = concurrent_renames(c_first);
        assert_eq!(na, nb, "A와 B가 다른 결과");
        assert_eq!(na, nc, "C가 다른 결과 (A 먼저: {c_first})");
        assert_eq!(na.len(), 3);
        // 둘 중 하나의 변경만 살아남는다(같은 버전이면 해시가 큰 쪽).
        let renamed = na.iter().filter(|n| n.ends_with("-renamed")).count();
        assert_eq!(renamed, 1, "{na:?}");
    }
}

#[test]
fn rename_propagates_and_persists() {
    let (a, b, c) = trio();
    let update = a.app.rename("laptop-2", T0 + 10).unwrap();
    assert_eq!(
        deliver(&b, &update.bytes, T0 + 11),
        Ok(Incoming::DevicesChanged)
    );
    assert_eq!(
        deliver(&c, &update.bytes, T0 + 11),
        Ok(Incoming::DevicesChanged)
    );
    for p in [&a, &b, &c] {
        assert!(names(p).contains(&"laptop-2".to_string()));
    }
    // 새 이름으로 보낸 텍스트의 보낸 이름도 바뀐다.
    let sealed = a.app.seal_text("hi", T0 + 12).unwrap();
    assert_eq!(
        clip(deliver(&b, &sealed.bytes, T0 + 12)).from_name,
        "laptop-2"
    );
    // 다시 열어도 유지
    let reopened = open(a.dir.path(), &a.secrets);
    assert_eq!(reopened.status().unwrap().device_name, "laptop-2");

    assert!(matches!(a.app.rename("", T0), Err(Error::BadName)));
    assert!(matches!(
        a.app.rename(&"x".repeat(65), T0),
        Err(Error::BadName)
    ));
}

#[test]
fn remove_device_errors() {
    let (a, _b, c) = trio();
    let me = a.app.status().unwrap().device_id.short_hex();
    assert!(matches!(
        a.app.remove_device("laptop", T0),
        Err(Error::CannotRemoveSelf)
    ));
    assert!(matches!(
        a.app.remove_device(&me, T0),
        Err(Error::CannotRemoveSelf)
    ));
    assert!(matches!(
        a.app.remove_device("nobody", T0),
        Err(Error::UnknownDevice)
    ));
    assert_eq!(
        a.app.devices().len(),
        3,
        "실패한 제거는 아무것도 바꾸지 않는다"
    );
    assert_eq!(a.app.status().unwrap().epoch, Some(1));

    // id 앞자리로도 지정할 수 있다.
    let victim = c.app.status().unwrap().device_id.short_hex();
    a.app.remove_device(&victim, T0).unwrap();
    assert!(a.app.devices().iter().all(|d| d.name != "tablet"));

    let no_group = profile("x", false);
    assert!(matches!(
        no_group.app.remove_device("y", T0),
        Err(Error::NoGroup)
    ));
}

#[test]
fn state_survives_restart_after_rotation() {
    let (a, _b, c) = trio();
    let _ = a.app.remove_device("phone", T0 + 1).unwrap();

    let reopened = open(a.dir.path(), &a.secrets);
    assert_eq!(reopened.status().unwrap().epoch, Some(2));
    assert_eq!(reopened.subscriptions().len(), 2);

    // 다시 열린 기기도 돌아온 기기의 질의에 응답한다.
    let query = c.app.seal_epoch_query(T0 + 5).unwrap();
    assert!(matches!(
        reopened.handle_deliver(&query.bytes, Source::Relay, T0 + 5),
        Ok(Incoming::Replies(_))
    ));
}

/// A가 C에게 줄 `Rekey`를 직접 만든다(잘못된 값을 넣어 보기 위해).
fn crafted_rekey(a: &Profile, c: &Profile, new_epoch: u64, key_epoch: u64) -> Vec<u8> {
    let slot = a.app.device.lock().unwrap();
    let dev = slot.as_ref().unwrap();
    let roster = dev.state.roster.clone().unwrap();
    let c_id = c.app.status().unwrap().device_id;
    let c_member = member_by_id(&roster, &c_id.0).unwrap().clone();
    let key = GroupKey::generate();
    let wrapped = wrap_key_for(&c_member, &key, key_epoch).unwrap();
    let new_roster = pb::Roster {
        epoch: new_epoch,
        version: roster.version + 1,
        members: roster.members.clone(),
    };
    seal_message(
        &dev.keys,
        &dev.send_context().unwrap(),
        pb::payload_body::Kind::Control(pb::Control {
            kind: Some(pb::control::Kind::Rekey(pb::Rekey {
                new_epoch,
                new_roster: Some(new_roster),
                keys: vec![wrapped],
            })),
        }),
        T0 + 1,
    )
    .unwrap()
    .bytes
}

#[test]
fn malformed_rekeys_are_ignored() {
    let (a, _b, c) = trio();

    // 키 안의 에포크가 Rekey가 말하는 에포크와 다르다.
    let wrong_key_epoch = crafted_rekey(&a, &c, 2, 7);
    assert_eq!(deliver(&c, &wrong_key_epoch, T0 + 1), Ok(Incoming::Nothing));
    assert_eq!(c.app.status().unwrap().epoch, Some(1));

    // 에포크가 올라가지 않는다.
    let not_newer = crafted_rekey(&a, &c, 1, 1);
    assert_eq!(deliver(&c, &not_newer, T0 + 1), Ok(Incoming::Nothing));
    assert_eq!(c.app.status().unwrap().epoch, Some(1));

    // 올바른 것은 받아들인다 (대조군).
    let valid = crafted_rekey(&a, &c, 2, 2);
    assert_eq!(deliver(&c, &valid, T0 + 1), Ok(Incoming::EpochChanged(2)));
    assert_eq!(c.app.status().unwrap().epoch, Some(2));
}
