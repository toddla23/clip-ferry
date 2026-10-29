//! 기기 이름 변경·제거, 키 회전(`Rekey`), 이전 에포크 유예, 돌아온 기기 복구(`EpochQuery`).
//! 규격은 docs/03-protocol.md 3절.

use zeroize::Zeroizing;

use super::*;
use crate::crypto::{unwrap, wrap_for, Wrapped};
use crate::envelope::seal_message;
use pb::control::Kind as C;
use pb::payload_body::Kind;

/// 이전 에포크 키를 보관하는 기간(30일). 이 기간이 지나면 지운다.
pub const GRACE_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// 연결 하나가 구독할 수 있는 채널 수(릴레이 기본 한도). 현재 에포크가 먼저, 그다음 최신 이전 에포크 순.
pub const MAX_SUBSCRIPTIONS: usize = 8;
/// 접촉 시각을 파일에 다시 쓰는 최소 간격(디스크 쓰기를 줄이기 위해).
const CONTACT_PERSIST_MS: u64 = 60 * 60 * 1000;

impl Device {
    pub(super) fn health(&self, now_ms: u64) -> GroupHealth {
        if self.removed {
            return GroupHealth::Removed;
        }
        let members = self.state.roster.as_ref().map_or(0, |r| r.members.len());
        if members > 1 && now_ms.saturating_sub(self.last_contact_ms) > GRACE_MS {
            GroupHealth::Stale
        } else {
            GroupHealth::Active
        }
    }
}

/// `group_key(32) ‖ epoch(8, LE)`
fn key_payload(key: &GroupKey, epoch: u64) -> Zeroizing<Vec<u8>> {
    let mut p = Zeroizing::new(Vec::with_capacity(40));
    p.extend_from_slice(key.as_bytes());
    p.extend_from_slice(&epoch.to_le_bytes());
    p
}

pub(super) fn wrap_key_for(
    member: &pb::Member,
    key: &GroupKey,
    epoch: u64,
) -> Result<pb::WrappedKey, Error> {
    let dh_pub: [u8; 32] = member
        .dh_pub
        .as_slice()
        .try_into()
        .map_err(|_| Error::BadGroup)?;
    let id: [u8; 16] = member
        .device_id
        .as_slice()
        .try_into()
        .map_err(|_| Error::BadGroup)?;
    let w = wrap_for(&dh_pub, &DeviceId(id), &key_payload(key, epoch))?;
    Ok(pb::WrappedKey {
        device_id: id.to_vec(),
        eph_pub: w.eph_pub.to_vec(),
        nonce: w.nonce.to_vec(),
        ct: w.ciphertext,
    })
}

fn unwrap_key(keys: &DeviceKeys, wk: &pb::WrappedKey) -> Option<(GroupKey, u64)> {
    let wrapped = Wrapped {
        eph_pub: wk.eph_pub.as_slice().try_into().ok()?,
        nonce: wk.nonce.as_slice().try_into().ok()?,
        ciphertext: wk.ct.clone(),
    };
    let plain = Zeroizing::new(unwrap(keys, &wrapped).ok()?);
    if plain.len() != 40 {
        return None;
    }
    let key = GroupKey::from_bytes(plain[..32].try_into().ok()?);
    let epoch = u64::from_le_bytes(plain[32..].try_into().ok()?);
    Some((key, epoch))
}

pub(super) fn member_by_id<'a>(roster: &'a pb::Roster, id: &[u8]) -> Option<&'a pb::Member> {
    roster.members.iter().find(|m| m.device_id == id)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 이름이 정확히 같거나, 아니면 id의 앞자리(hex 4자 이상)가 같은 구성원.
fn find_member(roster: &pb::Roster, target: &str) -> Result<pb::Member, Error> {
    let by_name: Vec<_> = roster.members.iter().filter(|m| m.name == target).collect();
    let matches = if by_name.is_empty() {
        let prefix = target.to_ascii_lowercase();
        if prefix.len() < 4 {
            return Err(Error::UnknownDevice);
        }
        roster
            .members
            .iter()
            .filter(|m| hex(&m.device_id).starts_with(&prefix))
            .collect()
    } else {
        by_name
    };
    match matches.as_slice() {
        [] => Err(Error::UnknownDevice),
        [one] => Ok((*one).clone()),
        _ => Err(Error::AmbiguousDevice),
    }
}

impl ClipFerry {
    /// 다른 구성원에게서 유효한 메시지를 받았다. 일정 간격으로만 파일에 적는다.
    pub(super) fn touch(&self, dev: &mut Device, now_ms: u64) {
        dev.last_contact_ms = now_ms;
        if now_ms.saturating_sub(dev.persisted_contact_ms) > CONTACT_PERSIST_MS {
            let mut state = dev.state.clone();
            state.last_contact_ms = now_ms;
            if store::write_state(&self.state_path(), &dev.state_key, &state).is_ok() {
                dev.state = state;
                dev.persisted_contact_ms = now_ms;
            }
        }
    }

    /// 새 에포크로 넘어간다. 지금 에포크는 유예 목록으로 옮기고 키는 계속 보관한다.
    fn install_epoch(
        &self,
        dev: &mut Device,
        epoch: u64,
        key: GroupKey,
        roster: pb::Roster,
        now_ms: u64,
    ) -> Result<(), Error> {
        let old_roster = dev.state.roster.clone().ok_or(Error::NoGroup)?;
        let old_key = dev.group_key.clone().ok_or(Error::NoGroup)?;
        let old_epoch = old_roster.epoch;

        let mut state = dev.state.clone();
        state.retired.insert(
            0,
            pb::RetiredEpoch {
                epoch: old_epoch,
                retired_at_ms: now_ms,
                roster: Some(old_roster),
            },
        );
        state.roster = Some(roster);
        state.last_contact_ms = now_ms;
        state.status = 0;

        self.secrets.set(&group_key_name(epoch), key.as_bytes())?;
        if let Err(e) = store::write_state(&self.state_path(), &dev.state_key, &state) {
            let _ = self.secrets.delete(&group_key_name(epoch));
            return Err(e.into());
        }
        dev.state = state;
        dev.retired_keys.insert(old_epoch, old_key);
        dev.group_key = Some(key);
        dev.last_contact_ms = now_ms;
        dev.persisted_contact_ms = now_ms;
        dev.removed = false;
        dev.sync_receiver();
        Ok(())
    }

    fn mark_removed(&self, dev: &mut Device) {
        dev.removed = true;
        let mut state = dev.state.clone();
        state.status = 1;
        if store::write_state(&self.state_path(), &dev.state_key, &state).is_ok() {
            dev.state = state;
        }
    }

    /// 받은 `Rekey`: 내 몫을 풀어 새 에포크로 넘어가거나, 내가 빠진 것을 알아챈다.
    pub(super) fn handle_rekey(
        &self,
        dev: &mut Device,
        sender: DeviceId,
        epoch: u64,
        rekey: pb::Rekey,
        now_ms: u64,
    ) -> Incoming {
        let current = dev.state.roster.as_ref().map_or(0, |r| r.epoch);
        // 내가 지금 쓰는 에포크의 채널로 온, 더 새 에포크로의 전환만 다룬다.
        if epoch != current || rekey.new_epoch <= current {
            return Incoming::Nothing;
        }
        let me = dev.keys.device_id();

        let Some(new_roster) = rekey.new_roster else {
            // 로스터 없이 내 id만 적힌 빈 항목 = "당신은 지금 그룹에 없다".
            // 같은 채널의 다른 기기에게 간 알림은 무시한다.
            if rekey
                .keys
                .iter()
                .any(|k| k.device_id == me.0 && k.ct.is_empty())
            {
                self.mark_removed(dev);
                return Incoming::Removed;
            }
            return Incoming::Nothing;
        };
        let Some(mine) = rekey.keys.iter().find(|k| k.device_id == me.0) else {
            if member_by_id(&new_roster, &me.0).is_none() {
                self.mark_removed(dev);
                return Incoming::Removed;
            }
            // 로스터에는 있는데 내 몫이 없다: 다른 응답을 기다린다.
            return Incoming::Nothing;
        };
        let Some((key, key_epoch)) = unwrap_key(&dev.keys, mine) else {
            return Incoming::Nothing;
        };

        let me_ok = member_by_id(&new_roster, &me.0).is_some_and(|m| {
            m.sign_pub == dev.keys.sign_public() && m.dh_pub == dev.keys.dh_public()
        });
        if key_epoch != rekey.new_epoch
            || new_roster.epoch != rekey.new_epoch
            || !roster_is_consistent(&new_roster)
            || !me_ok
            || member_by_id(&new_roster, &sender.0).is_none()
        {
            return Incoming::Nothing;
        }

        match self.install_epoch(dev, rekey.new_epoch, key, new_roster, now_ms) {
            Ok(()) => Incoming::EpochChanged(rekey.new_epoch),
            Err(_) => Incoming::Nothing,
        }
    }

    /// 이전 에포크 채널로 온 `EpochQuery`에 응답한다.
    /// 지금도 그룹에 있는 기기에게는 최신 키를, 제거된 기기에게는 "제거됨" 알림을 보낸다.
    pub(super) fn handle_query(
        &self,
        dev: &mut Device,
        sender: DeviceId,
        epoch: u64,
        now_ms: u64,
    ) -> Incoming {
        match self.reply_for(dev, sender, epoch, now_ms) {
            Some(m) => Incoming::Replies(vec![m]),
            None => Incoming::Nothing,
        }
    }

    /// `epoch`(이전 에포크)의 채널에서 듣고 있는 `member`에게 줄 응답을 `epoch`의 키로 봉인한다.
    fn reply_for(
        &self,
        dev: &Device,
        member: DeviceId,
        epoch: u64,
        now_ms: u64,
    ) -> Option<SealedMessage> {
        let (Some(roster), Some(current_key)) = (&dev.state.roster, &dev.group_key) else {
            return None;
        };
        if epoch >= roster.epoch {
            return None;
        }
        let old_key = dev.retired_keys.get(&epoch)?;
        let old_roster = dev
            .state
            .retired
            .iter()
            .find(|r| r.epoch == epoch)
            .and_then(|r| r.roster.as_ref())?;

        let rekey = match member_by_id(roster, &member.0) {
            Some(m) => pb::Rekey {
                new_epoch: roster.epoch,
                new_roster: Some(roster.clone()),
                keys: vec![wrap_key_for(m, current_key, roster.epoch).ok()?],
            },
            // 제거된 기기: 로스터는 보여 주지 않고, 누구에게 하는 말인지만 적는다.
            None => pb::Rekey {
                new_epoch: roster.epoch,
                new_roster: None,
                keys: vec![pb::WrappedKey {
                    device_id: member.0.to_vec(),
                    ..Default::default()
                }],
            },
        };
        let ctx = EpochContext::new(old_key, epoch, roster_sign_pubs(old_roster));
        let out = seal_message(
            &dev.keys,
            &ctx,
            Kind::Control(pb::Control {
                kind: Some(C::Rekey(rekey)),
            }),
            now_ms,
        )
        .ok()?;
        Some(SealedMessage {
            channel_id: ctx.channel_id().clone(),
            msg_id: out.msg_id,
            bytes: out.bytes,
        })
    }

    /// 이전 에포크 채널에 지금 붙어 있는 기기들(`online_tags`)에게 먼저 최신 키(또는 제거 알림)를 보낸다.
    /// 질의를 기다리지 않으므로, 한 번만 접속하는 기기(예: CLI 명령)가 있어도 따라잡을 수 있다.
    pub fn catch_up_replies(
        &self,
        epoch: u64,
        online_tags: &[Vec<u8>],
        now_ms: u64,
    ) -> Vec<SealedMessage> {
        let slot = self.device.lock().unwrap();
        let Some(dev) = slot.as_ref() else {
            return Vec::new();
        };
        if dev.removed {
            return Vec::new();
        }
        let (Some(old_key), Some(old_roster)) = (
            dev.retired_keys.get(&epoch),
            dev.state
                .retired
                .iter()
                .find(|r| r.epoch == epoch)
                .and_then(|r| r.roster.as_ref()),
        ) else {
            return Vec::new();
        };
        let me = dev.keys.device_id();
        old_roster
            .members
            .iter()
            .filter_map(|m| {
                let id = DeviceId(m.device_id.as_slice().try_into().ok()?);
                if id == me {
                    return None;
                }
                let tag = old_key.device_tag(epoch, &id);
                if !online_tags.iter().any(|t| t.as_slice() == tag.0) {
                    return None;
                }
                self.reply_for(dev, id, epoch, now_ms)
            })
            .collect()
    }

    /// 연결할 때 게시하는 "나는 이 에포크까지 안다" 질의. 더 새 에포크를 가진 기기가 응답한다.
    /// 같은 에포크의 기기에게는 연락이 닿았다는 신호(생존 확인)도 된다.
    pub fn seal_epoch_query(&self, now_ms: u64) -> Result<SealedMessage, Error> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        if dev.removed {
            return Err(Error::Removed);
        }
        let ctx = dev.send_context()?;
        let out = seal_message(
            &dev.keys,
            &ctx,
            Kind::Control(pb::Control {
                kind: Some(C::EpochQuery(pb::EpochQuery {
                    known_epoch: ctx.epoch(),
                })),
            }),
            now_ms,
        )?;
        Ok(SealedMessage {
            channel_id: ctx.channel_id().clone(),
            msg_id: out.msg_id,
            bytes: out.bytes,
        })
    }

    /// 이 기기의 이름을 바꾸고 `RosterUpdate`를 만든다.
    pub fn rename(&self, new_name: &str, now_ms: u64) -> Result<SealedMessage, Error> {
        if new_name.is_empty() || new_name.len() > 64 {
            return Err(Error::BadName);
        }
        let mut slot = self.device.lock().unwrap();
        let dev = slot.as_mut().ok_or(Error::NotInitialized)?;
        if dev.removed {
            return Err(Error::Removed);
        }
        let mut roster = dev.state.roster.clone().ok_or(Error::NoGroup)?;
        let me = dev.keys.device_id();
        let member = roster
            .members
            .iter_mut()
            .find(|m| m.device_id == me.0)
            .ok_or(Error::BadGroup)?;
        member.name = new_name.to_string();
        roster.version += 1;

        let mut state = dev.state.clone();
        state.device_name = new_name.to_string();
        state.roster = Some(roster.clone());
        store::write_state(&self.state_path(), &dev.state_key, &state)?;
        dev.state = state;
        dev.sync_receiver();

        let ctx = dev.send_context()?;
        let out = seal_message(
            &dev.keys,
            &ctx,
            Kind::Control(pb::Control {
                kind: Some(C::RosterUpdate(pb::RosterUpdate {
                    roster: Some(roster),
                })),
            }),
            now_ms,
        )?;
        Ok(SealedMessage {
            channel_id: ctx.channel_id().clone(),
            msg_id: out.msg_id,
            bytes: out.bytes,
        })
    }

    /// 기기를 제거한다: 새 그룹 키와 에포크를 만들고, 남은 기기마다 키를 래핑한 `Rekey`를
    /// **이전 채널**에 게시할 메시지로 돌려준다. 이 기기는 곧바로 새 에포크로 넘어간다.
    pub fn remove_device(&self, target: &str, now_ms: u64) -> Result<Vec<SealedMessage>, Error> {
        let mut slot = self.device.lock().unwrap();
        let dev = slot.as_mut().ok_or(Error::NotInitialized)?;
        if dev.removed {
            return Err(Error::Removed);
        }
        let roster = dev.state.roster.clone().ok_or(Error::NoGroup)?;
        let me = dev.keys.device_id();

        let victim = find_member(&roster, target)?;
        if victim.device_id == me.0 {
            return Err(Error::CannotRemoveSelf);
        }
        let new_epoch = roster.epoch + 1;
        let new_roster = pb::Roster {
            epoch: new_epoch,
            version: roster.version + 1,
            members: roster
                .members
                .iter()
                .filter(|m| m.device_id != victim.device_id)
                .cloned()
                .collect(),
        };
        let new_key = GroupKey::generate();
        let keys = new_roster
            .members
            .iter()
            .filter(|m| m.device_id != me.0)
            .map(|m| wrap_key_for(m, &new_key, new_epoch))
            .collect::<Result<Vec<_>, _>>()?;

        // 이전 에포크 맥락으로 봉인해 옛 채널의 모든 구성원이 읽게 한다.
        let old_ctx = dev.send_context()?;
        let out = seal_message(
            &dev.keys,
            &old_ctx,
            Kind::Control(pb::Control {
                kind: Some(C::Rekey(pb::Rekey {
                    new_epoch,
                    new_roster: Some(new_roster.clone()),
                    keys,
                })),
            }),
            now_ms,
        )?;
        self.install_epoch(dev, new_epoch, new_key, new_roster, now_ms)?;
        Ok(vec![SealedMessage {
            channel_id: old_ctx.channel_id().clone(),
            msg_id: out.msg_id,
            bytes: out.bytes,
        }])
    }

    /// 유예 기간(30일)이 지난 이전 에포크의 키를 지운다. 바뀐 게 있으면 `true`.
    pub fn prune_expired(&self, now_ms: u64) -> bool {
        let mut slot = self.device.lock().unwrap();
        let Some(dev) = slot.as_mut() else {
            return false;
        };
        let expired: Vec<u64> = dev
            .state
            .retired
            .iter()
            .filter(|r| now_ms.saturating_sub(r.retired_at_ms) > GRACE_MS)
            .map(|r| r.epoch)
            .collect();
        if expired.is_empty() {
            return false;
        }
        let mut state = dev.state.clone();
        state.retired.retain(|r| !expired.contains(&r.epoch));
        if store::write_state(&self.state_path(), &dev.state_key, &state).is_err() {
            return false;
        }
        dev.state = state;
        for epoch in expired {
            dev.retired_keys.remove(&epoch);
            let _ = self.secrets.delete(&group_key_name(epoch));
            if let Some(receiver) = dev.receiver.as_mut() {
                receiver.remove_epoch(epoch);
            }
        }
        true
    }

    /// 그룹과의 관계. 제거되었거나(`Removed`), 30일 넘게 연락이 없으면(`Stale`) 다시 페어링해야 한다.
    pub fn group_health(&self, now_ms: u64) -> GroupHealth {
        let slot = self.device.lock().unwrap();
        slot.as_ref()
            .map_or(GroupHealth::Active, |d| d.health(now_ms))
    }

    /// 지금 구독해야 하는 채널들: 현재 에포크, 그다음 유예 중인 이전 에포크(최신 순). 최대 8개.
    pub fn subscriptions(&self) -> Vec<Subscription> {
        let slot = self.device.lock().unwrap();
        let Some(dev) = slot.as_ref() else {
            return Vec::new();
        };
        if dev.removed {
            return Vec::new();
        }
        let me = dev.keys.device_id();
        let mut out = Vec::new();
        if let (Some(roster), Some(key)) = (&dev.state.roster, &dev.group_key) {
            out.push(Subscription {
                epoch: roster.epoch,
                channel_id: key.channel_id(roster.epoch),
                device_tag: key.device_tag(roster.epoch, &me),
            });
        }
        for r in &dev.state.retired {
            if let Some(key) = dev.retired_keys.get(&r.epoch) {
                out.push(Subscription {
                    epoch: r.epoch,
                    channel_id: key.channel_id(r.epoch),
                    device_tag: key.device_tag(r.epoch, &me),
                });
            }
        }
        out.truncate(MAX_SUBSCRIPTIONS);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster(names: &[&str]) -> pb::Roster {
        pb::Roster {
            epoch: 1,
            version: 1,
            members: names
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let keys = DeviceKeys::generate();
                    let mut m = crate::group::self_member(&keys, n, 0);
                    // 앞 4바이트가 서로 다르고 읽기 쉽도록 고정한다.
                    m.device_id = vec![
                        0xa0 + i as u8,
                        0xb0,
                        0xc0,
                        0xd0,
                        1,
                        2,
                        3,
                        4,
                        5,
                        6,
                        7,
                        8,
                        9,
                        10,
                        11,
                        12,
                    ];
                    m
                })
                .collect(),
        }
    }

    #[test]
    fn find_member_by_name_or_id_prefix() {
        let r = roster(&["laptop", "phone", "tablet"]);
        assert_eq!(find_member(&r, "phone").unwrap().name, "phone");
        assert_eq!(find_member(&r, "a1b0c0d0").unwrap().name, "phone");
        assert_eq!(find_member(&r, "A1B0").unwrap().name, "phone");
        assert!(matches!(find_member(&r, "nope"), Err(Error::UnknownDevice)));
        // 4자 미만의 id 앞자리는 인정하지 않는다.
        assert!(matches!(find_member(&r, "a1"), Err(Error::UnknownDevice)));
        assert!(matches!(find_member(&r, ""), Err(Error::UnknownDevice)));
    }

    #[test]
    fn find_member_rejects_ambiguity() {
        let mut r = roster(&["phone", "phone", "tablet"]);
        assert!(matches!(
            find_member(&r, "phone"),
            Err(Error::AmbiguousDevice)
        ));
        // id 앞자리가 겹치면 모호하다.
        r.members[1].device_id[0] = r.members[0].device_id[0];
        r.members[1].name = "other".into();
        assert!(matches!(
            find_member(&r, "a0b0"),
            Err(Error::AmbiguousDevice)
        ));
        // 이름이 정확히 하나만 맞으면 id 앞자리와 겹쳐도 이름이 우선이다.
        assert_eq!(find_member(&r, "other").unwrap().name, "other");
    }

    #[test]
    fn key_payload_roundtrip_and_rejects_bad_input() {
        let me = DeviceKeys::generate();
        let member = crate::group::self_member(&me, "me", 0);
        let key = GroupKey::from_bytes([5; 32]);
        let wk = wrap_key_for(&member, &key, 7).unwrap();
        let (got, epoch) = unwrap_key(&me, &wk).unwrap();
        assert_eq!(got.as_bytes(), &[5; 32]);
        assert_eq!(epoch, 7);

        let other = DeviceKeys::generate();
        assert!(unwrap_key(&other, &wk).is_none());
        let mut bad = wk.clone();
        bad.ct[0] ^= 1;
        assert!(unwrap_key(&me, &bad).is_none());
        let mut short = wk;
        short.nonce.pop();
        assert!(unwrap_key(&me, &short).is_none());
    }
}
