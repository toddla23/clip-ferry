//! `PairWelcome`: 새 기기에게 그룹 키와 로스터를 건네는 메시지.

use prost::Message;

use crate::crypto::{
    unwrap, verify_signature, wrap_for, CryptoError, DeviceId, DeviceKeys, GroupKey, Wrapped,
    NONCE_LEN,
};
use crate::pb;

const SIG_DOMAIN: &[u8] = b"clip-ferry v1 welcome";
/// 평문 앞부분: `group_key(32) ‖ epoch(8, LE)`
const FIXED_LEN: usize = 32 + 8;

/// 참여하는 기기의 공개 정보.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinerInfo {
    pub name: String,
    pub sign_pub: [u8; 32],
    pub dh_pub: [u8; 32],
}

impl JoinerInfo {
    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_sign_pub(&self.sign_pub)
    }
}

/// 참여가 끝나 받은 그룹 정보.
#[derive(Debug)]
pub struct JoinedGroup {
    pub group_key: GroupKey,
    pub epoch: u64,
    pub roster: pb::Roster,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WelcomeError {
    #[error("초대한 기기의 서명이 맞지 않습니다")]
    BadSignature,
    #[error("환영 메시지를 열 수 없습니다")]
    Undecryptable,
    #[error("환영 메시지의 내용이 올바르지 않습니다")]
    BadContents,
}

fn signature_input(w: &pb::WrappedKey) -> Vec<u8> {
    let mut m = SIG_DOMAIN.to_vec();
    m.extend_from_slice(&w.eph_pub);
    m.extend_from_slice(&w.nonce);
    m.extend_from_slice(&w.ct);
    m
}

/// `wrap_for(joiner, group_key ‖ epoch ‖ roster)`를 만들고 `inviter`가 서명한다.
pub(crate) fn build(
    inviter: &DeviceKeys,
    joiner: &JoinerInfo,
    group_key: &GroupKey,
    roster: &pb::Roster,
) -> Result<pb::PairWelcome, CryptoError> {
    let mut plain = Vec::with_capacity(FIXED_LEN + 64 * roster.members.len());
    plain.extend_from_slice(group_key.as_bytes());
    plain.extend_from_slice(&roster.epoch.to_le_bytes());
    plain.extend_from_slice(&roster.encode_to_vec());

    let joiner_id = joiner.device_id();
    let wrapped = wrap_for(&joiner.dh_pub, &joiner_id, &plain)?;
    let key = pb::WrappedKey {
        device_id: joiner_id.0.to_vec(),
        eph_pub: wrapped.eph_pub.to_vec(),
        nonce: wrapped.nonce.to_vec(),
        ct: wrapped.ciphertext,
    };
    let signature = inviter.sign(&signature_input(&key)).to_vec();
    Ok(pb::PairWelcome {
        wrapped: Some(key),
        signature,
    })
}

/// 환영 메시지를 검증하고 연다. `inviter_sign_pub`은 `PairOffer`로 받은 값이다.
pub(crate) fn open(
    me: &DeviceKeys,
    inviter_sign_pub: &[u8; 32],
    welcome: &pb::PairWelcome,
) -> Result<JoinedGroup, WelcomeError> {
    let key = welcome.wrapped.as_ref().ok_or(WelcomeError::BadContents)?;
    verify_signature(inviter_sign_pub, &signature_input(key), &welcome.signature)
        .map_err(|_| WelcomeError::BadSignature)?;
    if key.device_id != me.device_id().0 {
        return Err(WelcomeError::BadContents);
    }

    let wrapped = Wrapped {
        eph_pub: key
            .eph_pub
            .as_slice()
            .try_into()
            .map_err(|_| WelcomeError::BadContents)?,
        nonce: key
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_: std::array::TryFromSliceError| WelcomeError::BadContents)
            .map(|n: [u8; NONCE_LEN]| n)?,
        ciphertext: key.ct.clone(),
    };
    let plain =
        zeroize::Zeroizing::new(unwrap(me, &wrapped).map_err(|_| WelcomeError::Undecryptable)?);
    if plain.len() < FIXED_LEN {
        return Err(WelcomeError::BadContents);
    }
    let group_key = GroupKey::from_bytes(plain[..32].try_into().expect("길이 확인됨"));
    let epoch = u64::from_le_bytes(plain[32..FIXED_LEN].try_into().expect("길이 확인됨"));
    let roster = pb::Roster::decode(&plain[FIXED_LEN..]).map_err(|_| WelcomeError::BadContents)?;

    // 로스터가 약속한 내용과 맞는지: 에포크 일치, 내가 들어 있고, 초대한 기기도 들어 있음.
    let me_ok = roster.members.iter().any(|m| {
        m.device_id == me.device_id().0
            && m.sign_pub == me.sign_public()
            && m.dh_pub == me.dh_public()
    });
    let inviter_ok = roster
        .members
        .iter()
        .any(|m| m.sign_pub == inviter_sign_pub.as_slice());
    if roster.epoch != epoch || epoch == 0 || !me_ok || !inviter_ok {
        return Err(WelcomeError::BadContents);
    }
    Ok(JoinedGroup {
        group_key,
        epoch,
        roster,
    })
}
