//! 그룹 생성과 로스터 기본.

use crate::crypto::{DeviceKeys, GroupKey};
use crate::pb;

/// 첫 에포크 번호.
pub const FIRST_EPOCH: u64 = 1;

/// 이 기기만 들어 있는 `Member`.
pub fn self_member(keys: &DeviceKeys, name: &str, now_ms: u64) -> pb::Member {
    pb::Member {
        device_id: keys.device_id().0.to_vec(),
        sign_pub: keys.sign_public().to_vec(),
        dh_pub: keys.dh_public().to_vec(),
        name: name.to_string(),
        added_at_ms: now_ms,
        push: None,
    }
}

/// 새 그룹: 무작위 그룹 키, 에포크 1, 이 기기만 있는 로스터.
pub fn create(keys: &DeviceKeys, name: &str, now_ms: u64) -> (GroupKey, pb::Roster) {
    let roster = pb::Roster {
        epoch: FIRST_EPOCH,
        version: 1,
        members: vec![self_member(keys, name, now_ms)],
    };
    (GroupKey::generate(), roster)
}
