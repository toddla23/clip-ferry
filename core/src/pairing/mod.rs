//! 페어링 (docs/03-protocol.md 4절).
//!
//! 입출력이 없는 상태 기계다. 호출하는 쪽이 릴레이로 받은 바이트를 `handle`에 넣고,
//! 돌려받은 바이트를 `invite.pair_channel()`에 게시한다.

mod invite;
#[cfg(test)]
mod tests;
mod welcome;

use prost::Message;
use sha2::{Digest, Sha256};

use crate::crypto::{self, ContentKey, DeviceKeys};
use crate::pb;

pub use invite::{Invite, InviteError, INVITE_TTL_MS};
pub(crate) use welcome::{build as build_welcome, open as open_welcome};
pub use welcome::{JoinedGroup, JoinerInfo, WelcomeError};

const AAD_PREFIX: &[u8] = b"clip-ferry/v1/pair";

/// 페어링 메시지를 `pair_key`로 봉인한다 (`PairEnvelope` 직렬화).
fn seal_message(invite: &Invite, msg: pb::pair_message::Kind) -> Vec<u8> {
    let key = ContentKey::from_bytes(invite.pair_key());
    let plain = pb::PairMessage { kind: Some(msg) }.encode_to_vec();
    let sealed = crypto::seal(&key, &plain, &aad(invite)).expect("봉인은 실패하지 않는다");
    pb::PairEnvelope {
        nonce: sealed.nonce.to_vec(),
        ct: sealed.ciphertext,
    }
    .encode_to_vec()
}

/// 열 수 없거나 형식이 맞지 않으면 `None` (같은 채널의 다른 초대나 잡음).
fn open_message(invite: &Invite, bytes: &[u8]) -> Option<pb::pair_message::Kind> {
    let env = pb::PairEnvelope::decode(bytes).ok()?;
    let nonce: [u8; crypto::NONCE_LEN] = env.nonce.as_slice().try_into().ok()?;
    let key = ContentKey::from_bytes(invite.pair_key());
    let plain = crypto::open(&key, &nonce, &env.ct, &aad(invite)).ok()?;
    pb::PairMessage::decode(plain.as_slice()).ok()?.kind
}

fn aad(invite: &Invite) -> Vec<u8> {
    let mut a = AAD_PREFIX.to_vec();
    a.extend_from_slice(&invite.pair_channel());
    a
}

/// 양쪽 화면에 보여 줄 6자리 확인 숫자.
/// `u32_le(SHA-256("clip-ferry v1 sas" ‖ invite_secret ‖ sign_pub_A ‖ sign_pub_B)[0..4]) mod 1_000_000`
pub fn sas(invite: &Invite, sign_pub_a: &[u8; 32], sign_pub_b: &[u8; 32]) -> String {
    let mut h = Sha256::new();
    h.update(b"clip-ferry v1 sas");
    h.update(invite.secret());
    h.update(sign_pub_a);
    h.update(sign_pub_b);
    let digest = h.finalize();
    let n = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]);
    format!("{:06}", n % 1_000_000)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PairingError {
    #[error("승인할 참여 요청이 없습니다")]
    NotPending,
    #[error("초대가 만료되었습니다")]
    Expired,
    #[error("초대가 더 이상 유효하지 않습니다")]
    Closed,
    #[error("이미 그룹에 있는 기기입니다")]
    AlreadyMember,
}

/// 초대가 무효가 된 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteProblem {
    Expired,
    AlreadyUsed,
    /// 다른 기기가 같은 초대로 또 참여를 시도했다. 초대를 무효화한다.
    SecondJoiner,
    Rejected,
}

/// 기기 공개 정보. 페어링 메시지에 실려 간다.
#[derive(Debug, Clone)]
pub(crate) struct Identity {
    pub sign_pub: [u8; 32],
    pub dh_pub: [u8; 32],
    pub name: String,
}

impl Identity {
    pub fn of(keys: &DeviceKeys, name: &str) -> Self {
        Self {
            sign_pub: keys.sign_public(),
            dh_pub: keys.dh_public(),
            name: name.to_string(),
        }
    }
}

#[derive(Debug)]
enum InviterState {
    Waiting,
    Pending(JoinerInfo),
    Approved,
    Closed(InviteProblem),
}

/// 초대한 쪽(A)의 상태.
pub struct Inviter {
    invite: Invite,
    created_ms: u64,
    me: Identity,
    state: InviterState,
}

/// `Inviter::handle`의 결과.
#[derive(Debug, PartialEq, Eq)]
pub enum InviterEvent {
    /// 무시한다.
    None,
    /// 참여 요청이 왔다. `payload`를 게시하고 `sas`를 사용자에게 보여 승인을 받는다.
    /// 같은 기기가 다시 보내면 같은 내용이 또 온다(재전송 대응).
    Offer {
        payload: Vec<u8>,
        joiner_name: String,
        sas: String,
    },
    /// 초대가 무효가 되었다 (사용자에게 경고).
    Invalid(InviteProblem),
}

impl Inviter {
    pub(crate) fn new(invite: Invite, me: Identity, now_ms: u64) -> Self {
        Self {
            invite,
            created_ms: now_ms,
            me,
            state: InviterState::Waiting,
        }
    }

    pub fn invite(&self) -> &Invite {
        &self.invite
    }

    pub fn expires_at_ms(&self) -> u64 {
        self.created_ms + INVITE_TTL_MS
    }

    fn expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms()
    }

    /// 승인을 기다리는 참여자.
    pub fn pending(&self) -> Option<&JoinerInfo> {
        match &self.state {
            InviterState::Pending(j) => Some(j),
            _ => None,
        }
    }

    /// 릴레이에서 받은 `pair_channel` 페이로드를 처리한다.
    pub fn handle(&mut self, payload: &[u8], now_ms: u64) -> InviterEvent {
        let Some(pb::pair_message::Kind::Hello(hello)) = open_message(&self.invite, payload) else {
            return InviterEvent::None;
        };
        let (Ok(sign_pub), Ok(dh_pub)) = (
            <[u8; 32]>::try_from(hello.sign_pub.as_slice()),
            <[u8; 32]>::try_from(hello.dh_pub.as_slice()),
        ) else {
            return InviterEvent::None;
        };

        match &self.state {
            InviterState::Approved => return InviterEvent::Invalid(InviteProblem::AlreadyUsed),
            // 이미 경고한 뒤로는 조용히 무시한다.
            InviterState::Closed(InviteProblem::SecondJoiner) => return InviterEvent::None,
            InviterState::Closed(why) => return InviterEvent::Invalid(*why),
            _ => {}
        }
        if self.expired(now_ms) {
            self.state = InviterState::Closed(InviteProblem::Expired);
            return InviterEvent::Invalid(InviteProblem::Expired);
        }

        let joiner = JoinerInfo {
            name: hello.name,
            sign_pub,
            dh_pub,
        };
        if let InviterState::Pending(first) = &self.state {
            if first.sign_pub != joiner.sign_pub || first.dh_pub != joiner.dh_pub {
                self.state = InviterState::Closed(InviteProblem::SecondJoiner);
                return InviterEvent::Invalid(InviteProblem::SecondJoiner);
            }
        }

        let sas = sas(&self.invite, &self.me.sign_pub, &joiner.sign_pub);
        let joiner_name = joiner.name.clone();
        self.state = InviterState::Pending(joiner);
        InviterEvent::Offer {
            payload: seal_message(
                &self.invite,
                pb::pair_message::Kind::Offer(pb::PairOffer {
                    sign_pub: self.me.sign_pub.to_vec(),
                    dh_pub: self.me.dh_pub.to_vec(),
                    name: self.me.name.clone(),
                }),
            ),
            joiner_name,
            sas,
        }
    }

    /// 사용자가 승인했다. 참여자 정보를 돌려주고 초대를 "사용함"으로 바꾼다.
    pub(crate) fn take_approved(&mut self, now_ms: u64) -> Result<JoinerInfo, PairingError> {
        match &self.state {
            InviterState::Pending(_) => {}
            InviterState::Waiting => return Err(PairingError::NotPending),
            InviterState::Approved | InviterState::Closed(_) => return Err(PairingError::Closed),
        }
        if self.expired(now_ms) {
            self.state = InviterState::Closed(InviteProblem::Expired);
            return Err(PairingError::Expired);
        }
        match std::mem::replace(&mut self.state, InviterState::Approved) {
            InviterState::Pending(j) => Ok(j),
            _ => unreachable!("위에서 확인함"),
        }
    }

    /// 환영 메시지를 만든 뒤 호출해 한 번 더 쓰이지 않게 한다. (`take_approved`가 이미 처리)
    pub(crate) fn welcome_payload(&self, welcome: pb::PairWelcome) -> Vec<u8> {
        seal_message(&self.invite, pb::pair_message::Kind::Welcome(welcome))
    }

    /// 사용자가 거절했다. 참여자에게 보낼 `PairReject` 페이로드를 돌려준다.
    pub fn reject(&mut self) -> Result<Vec<u8>, PairingError> {
        match &self.state {
            InviterState::Pending(_) | InviterState::Waiting => {}
            _ => return Err(PairingError::Closed),
        }
        self.state = InviterState::Closed(InviteProblem::Rejected);
        Ok(seal_message(
            &self.invite,
            pb::pair_message::Kind::Reject(pb::PairReject {}),
        ))
    }

    /// 만료되었을 때 참여자에게 알릴 `PairReject` 페이로드.
    pub fn expire(&mut self, now_ms: u64) -> Option<Vec<u8>> {
        if !self.expired(now_ms) || matches!(self.state, InviterState::Approved) {
            return None;
        }
        self.state = InviterState::Closed(InviteProblem::Expired);
        Some(seal_message(
            &self.invite,
            pb::pair_message::Kind::Reject(pb::PairReject {}),
        ))
    }
}

#[derive(Debug)]
enum JoinerState {
    Start,
    Offered { offer: pb::PairOffer },
    Done,
}

/// 참여하는 쪽(B)의 상태.
pub struct Joiner {
    invite: Invite,
    keys: DeviceKeys,
    me: Identity,
    state: JoinerState,
}

/// `Joiner::handle`의 결과.
#[derive(Debug)]
pub enum JoinerEvent {
    None,
    /// 초대한 기기가 응답했다. 같은 숫자가 상대 화면에도 떠 있는지 사용자가 확인한다.
    Offer {
        sas: String,
        inviter_name: String,
    },
    /// 참여 완료. 이 값을 `ClipFerry::accept_join`에 넘겨 저장한다.
    Joined(JoinedGroup),
    /// 초대한 쪽이 거절했거나 초대가 만료되었다.
    Rejected,
    /// 환영 메시지가 올바르지 않아 중단했다.
    Failed(WelcomeError),
}

impl Joiner {
    pub(crate) fn new(invite: Invite, keys: DeviceKeys, name: &str) -> Self {
        let me = Identity::of(&keys, name);
        Self {
            invite,
            keys,
            me,
            state: JoinerState::Start,
        }
    }

    pub fn invite(&self) -> &Invite {
        &self.invite
    }

    /// 보낼 `PairHello`. 응답이 올 때까지 여러 번 보내도 된다.
    pub fn hello(&self) -> Vec<u8> {
        seal_message(
            &self.invite,
            pb::pair_message::Kind::Hello(pb::PairHello {
                sign_pub: self.me.sign_pub.to_vec(),
                dh_pub: self.me.dh_pub.to_vec(),
                name: self.me.name.clone(),
            }),
        )
    }

    /// 릴레이에서 받은 `pair_channel` 페이로드를 처리한다.
    pub fn handle(&mut self, payload: &[u8]) -> JoinerEvent {
        let Some(kind) = open_message(&self.invite, payload) else {
            return JoinerEvent::None;
        };
        if matches!(self.state, JoinerState::Done) {
            return JoinerEvent::None;
        }
        match kind {
            pb::pair_message::Kind::Offer(offer) => {
                // 환영 메시지를 받은 뒤에는 서명 검증에 쓸 `offer`가 바뀌면 안 된다.
                let Ok(inviter_pub) = <[u8; 32]>::try_from(offer.sign_pub.as_slice()) else {
                    return JoinerEvent::None;
                };
                if offer.dh_pub.len() != 32 {
                    return JoinerEvent::None;
                }
                if let JoinerState::Offered { offer: first } = &self.state {
                    // 처음 응답한 기기만 인정한다.
                    if first.sign_pub != offer.sign_pub {
                        return JoinerEvent::None;
                    }
                }
                let sas = sas(&self.invite, &inviter_pub, &self.me.sign_pub);
                let inviter_name = offer.name.clone();
                let already = matches!(self.state, JoinerState::Offered { .. });
                self.state = JoinerState::Offered { offer };
                if already {
                    JoinerEvent::None
                } else {
                    JoinerEvent::Offer { sas, inviter_name }
                }
            }
            pb::pair_message::Kind::Welcome(welcome) => {
                let JoinerState::Offered { offer } = &self.state else {
                    // `Offer`보다 먼저 온 환영 메시지는 믿을 수 없다.
                    return JoinerEvent::None;
                };
                let Ok(inviter_pub) = <[u8; 32]>::try_from(offer.sign_pub.as_slice()) else {
                    return JoinerEvent::None;
                };
                match open_welcome(&self.keys, &inviter_pub, &welcome) {
                    Ok(joined) => {
                        self.state = JoinerState::Done;
                        JoinerEvent::Joined(joined)
                    }
                    Err(e) => JoinerEvent::Failed(e),
                }
            }
            pb::pair_message::Kind::Reject(_) => {
                self.state = JoinerState::Done;
                JoinerEvent::Rejected
            }
            pb::pair_message::Kind::Hello(_) => JoinerEvent::None,
        }
    }
}
