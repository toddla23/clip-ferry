//! 계정 연결 (docs/03-protocol.md 4.4, F37).
//!
//! 같은 계정으로 로그인한 새 기기가 QR·코드 없이 기존 기기에 참여를 요청한다.
//!
//! - 계정 채널 `account_channel = HKDF(user_id, "clip-ferry v1 account-channel")`에 봉인 없이 게시한다.
//! - 새 기기는 계정 서버가 서명한 참여 확인서(`LinkTicket`)를 `LinkRequest`에 실어 보낸다.
//!   기존 기기는 서명·계정·만료를 확인하므로 다른 계정의 기기는 요청을 보낼 수 없다.
//! - 기존 기기는 새 초대 비밀을 확인서의 `dh_pub`으로 래핑해 `LinkInvite`로 돌려준다.
//!   이후는 그 초대로 봉인 후 공개 페어링(`pairing`, 4.4)을 한다. 계정 서버가 키를 바꿔치기해도
//!   양쪽 확인 숫자가 달라진다.

use prost::Message;
use rand_core::{OsRng, RngCore};

use crate::crypto::{derive32, unwrap, verify_signature, wrap_for, DeviceId, DeviceKeys, Wrapped};
use crate::pairing::Invite;
use crate::pb;

/// 계정 서버 서명의 영역 구분 문자열.
pub const TICKET_DOMAIN: &[u8] = b"clip-ferry v1 link-ticket";

/// 계정 채널 ID.
pub fn account_channel(user_id: &str) -> [u8; 32] {
    derive32(user_id.as_bytes(), b"clip-ferry v1 account-channel")
}

/// 계정 서버가 서명하는 바이트:
/// `"clip-ferry v1 link-ticket" ‖ u8 len ‖ user_id ‖ sign_pub ‖ dh_pub ‖ u8 len ‖ name ‖ u64_le expires_at_ms`
/// (`user_id`·`name`은 UTF-8, 각각 255바이트 이하)
pub fn ticket_message(
    user_id: &str,
    sign_pub: &[u8; 32],
    dh_pub: &[u8; 32],
    name: &str,
    expires_at_ms: u64,
) -> Option<Vec<u8>> {
    let (u, n) = (user_id.as_bytes(), name.as_bytes());
    let (Ok(ul), Ok(nl)) = (u8::try_from(u.len()), u8::try_from(n.len())) else {
        return None;
    };
    let mut m = Vec::with_capacity(TICKET_DOMAIN.len() + 2 + u.len() + 64 + n.len() + 8);
    m.extend_from_slice(TICKET_DOMAIN);
    m.push(ul);
    m.extend_from_slice(u);
    m.extend_from_slice(sign_pub);
    m.extend_from_slice(dh_pub);
    m.push(nl);
    m.extend_from_slice(n);
    m.extend_from_slice(&expires_at_ms.to_le_bytes());
    Some(m)
}

/// 이 기기가 속한 계정과, 참여 확인서를 확인할 계정 서버 공개키들(`GET /v1/keys`의 `kid`, `x`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLink {
    pub user_id: String,
    pub server_keys: Vec<(String, [u8; 32])>,
}

impl AccountLink {
    pub fn channel(&self) -> [u8; 32] {
        account_channel(&self.user_id)
    }
}

/// 확인을 마친 참여 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRequestInfo {
    pub request_id: [u8; 16],
    pub name: String,
    pub sign_pub: [u8; 32],
    pub dh_pub: [u8; 32],
    pub expires_at_ms: u64,
}

impl LinkRequestInfo {
    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_sign_pub(&self.sign_pub)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    #[error("참여 확인서 형식이 올바르지 않습니다")]
    BadFormat,
    #[error("참여 확인서를 서명한 키를 모릅니다")]
    UnknownKey,
    #[error("참여 확인서 서명이 맞지 않습니다")]
    BadSignature,
    #[error("다른 계정의 기기가 보낸 요청입니다")]
    OtherAccount,
    #[error("참여 확인서가 만료되었습니다")]
    Expired,
}

/// 참여 확인서를 확인한다: 서명(계정 서버 키), 같은 계정, 만료.
pub fn verify_ticket(
    t: &pb::LinkTicket,
    link: &AccountLink,
    now_ms: u64,
) -> Result<([u8; 32], [u8; 32]), TicketError> {
    let sign_pub: [u8; 32] = t
        .sign_pub
        .as_slice()
        .try_into()
        .map_err(|_| TicketError::BadFormat)?;
    let dh_pub: [u8; 32] = t
        .dh_pub
        .as_slice()
        .try_into()
        .map_err(|_| TicketError::BadFormat)?;
    let msg = ticket_message(&t.user_id, &sign_pub, &dh_pub, &t.name, t.expires_at_ms)
        .ok_or(TicketError::BadFormat)?;
    let key = link
        .server_keys
        .iter()
        .find(|(kid, _)| *kid == t.kid)
        .map(|(_, k)| k)
        .ok_or(TicketError::UnknownKey)?;
    verify_signature(key, &msg, &t.signature).map_err(|_| TicketError::BadSignature)?;
    if t.user_id != link.user_id {
        return Err(TicketError::OtherAccount);
    }
    if now_ms >= t.expires_at_ms {
        return Err(TicketError::Expired);
    }
    Ok((sign_pub, dh_pub))
}

/// 계정 채널에서 받은 바이트가 참여 요청이면 확인해 돌려준다. 요청이 아니면 `None`.
pub fn parse_request(
    payload: &[u8],
    link: &AccountLink,
    now_ms: u64,
) -> Option<Result<LinkRequestInfo, TicketError>> {
    let Some(pb::link_message::Kind::Request(req)) = pb::LinkMessage::decode(payload).ok()?.kind
    else {
        return None;
    };
    Some((|| {
        let request_id: [u8; 16] = req
            .request_id
            .as_slice()
            .try_into()
            .map_err(|_| TicketError::BadFormat)?;
        let ticket = req.ticket.as_ref().ok_or(TicketError::BadFormat)?;
        let (sign_pub, dh_pub) = verify_ticket(ticket, link, now_ms)?;
        Ok(LinkRequestInfo {
            request_id,
            name: ticket.name.clone(),
            sign_pub,
            dh_pub,
            expires_at_ms: ticket.expires_at_ms,
        })
    })())
}

/// 새 기기가 게시할 참여 요청. 돌려주는 첫 값은 요청 ID(응답을 고를 때 쓴다).
pub fn build_request(ticket: pb::LinkTicket) -> ([u8; 16], Vec<u8>) {
    let mut request_id = [0u8; 16];
    OsRng.fill_bytes(&mut request_id);
    let bytes = pb::LinkMessage {
        kind: Some(pb::link_message::Kind::Request(pb::LinkRequest {
            request_id: request_id.to_vec(),
            ticket: Some(ticket),
        })),
    }
    .encode_to_vec();
    (request_id, bytes)
}

/// 기존 기기가 게시할 `LinkInvite`: 초대 비밀을 새 기기의 `dh_pub`으로 래핑한다.
pub(crate) fn build_invite(req: &LinkRequestInfo, invite: &Invite) -> Vec<u8> {
    let id = req.device_id();
    let w = wrap_for(&req.dh_pub, &id, invite.secret()).expect("래핑은 실패하지 않는다");
    pb::LinkMessage {
        kind: Some(pb::link_message::Kind::Invite(pb::LinkInvite {
            request_id: req.request_id.to_vec(),
            wrapped: Some(pb::WrappedKey {
                device_id: id.0.to_vec(),
                eph_pub: w.eph_pub.to_vec(),
                nonce: w.nonce.to_vec(),
                ct: w.ciphertext,
            }),
        })),
    }
    .encode_to_vec()
}

/// 내 요청(`request_id`)에 온 `LinkInvite`면 초대 비밀을 풀어 돌려준다.
pub(crate) fn open_invite(
    payload: &[u8],
    request_id: &[u8; 16],
    keys: &DeviceKeys,
) -> Option<[u8; 32]> {
    let Some(pb::link_message::Kind::Invite(inv)) = pb::LinkMessage::decode(payload).ok()?.kind
    else {
        return None;
    };
    if inv.request_id != request_id {
        return None;
    }
    let w = inv.wrapped?;
    if w.device_id != keys.device_id().0 {
        return None;
    }
    let wrapped = Wrapped {
        eph_pub: w.eph_pub.as_slice().try_into().ok()?,
        nonce: w.nonce.as_slice().try_into().ok()?,
        ciphertext: w.ct,
    };
    let plain = zeroize::Zeroizing::new(unwrap(keys, &wrapped).ok()?);
    plain.as_slice().try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn server() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn link(user: &str) -> AccountLink {
        AccountLink {
            user_id: user.into(),
            server_keys: vec![("k1".into(), server().verifying_key().to_bytes())],
        }
    }

    fn ticket(user: &str, keys: &DeviceKeys, expires: u64) -> pb::LinkTicket {
        let msg = ticket_message(
            user,
            &keys.sign_public(),
            &keys.dh_public(),
            "새 폰",
            expires,
        )
        .unwrap();
        pb::LinkTicket {
            user_id: user.into(),
            sign_pub: keys.sign_public().to_vec(),
            dh_pub: keys.dh_public().to_vec(),
            name: "새 폰".into(),
            expires_at_ms: expires,
            kid: "k1".into(),
            signature: server().sign(&msg).to_bytes().to_vec(),
        }
    }

    #[test]
    fn ticket_message_matches_fixed_vector() {
        // 계정 서버(account/src/link.rs)의 같은 이름 테스트와 같은 값이어야 한다.
        let m = ticket_message("u1", &[1u8; 32], &[2u8; 32], "ab", 0x0102).unwrap();
        let mut want = b"clip-ferry v1 link-ticket".to_vec();
        want.extend_from_slice(&[2, b'u', b'1']);
        want.extend_from_slice(&[1u8; 32]);
        want.extend_from_slice(&[2u8; 32]);
        want.extend_from_slice(&[2, b'a', b'b']);
        want.extend_from_slice(&[2, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(m, want);
    }

    #[test]
    fn request_round_trip_and_checks() {
        let b = DeviceKeys::generate();
        let (id, bytes) = build_request(ticket("u1", &b, 1000));
        let got = parse_request(&bytes, &link("u1"), 10).unwrap().unwrap();
        assert_eq!(got.request_id, id);
        assert_eq!(got.sign_pub, b.sign_public());
        assert_eq!(got.dh_pub, b.dh_public());
        assert_eq!(got.name, "새 폰");

        assert_eq!(
            parse_request(&bytes, &link("u2"), 10).unwrap(),
            Err(TicketError::OtherAccount)
        );
        assert_eq!(
            parse_request(&bytes, &link("u1"), 1000).unwrap(),
            Err(TicketError::Expired)
        );
        let mut other = link("u1");
        other.server_keys[0].1 = SigningKey::from_bytes(&[8u8; 32])
            .verifying_key()
            .to_bytes();
        assert_eq!(
            parse_request(&bytes, &other, 10).unwrap(),
            Err(TicketError::BadSignature)
        );
        other.server_keys[0].0 = "k2".into();
        assert_eq!(
            parse_request(&bytes, &other, 10).unwrap(),
            Err(TicketError::UnknownKey)
        );
    }

    #[test]
    fn tampered_ticket_is_rejected() {
        let b = DeviceKeys::generate();
        let mut t = ticket("u1", &b, 1000);
        // 계정 서버가 서명한 뒤 다른 키로 바꿔치기
        t.dh_pub = DeviceKeys::generate().dh_public().to_vec();
        let (_, bytes) = build_request(t);
        assert_eq!(
            parse_request(&bytes, &link("u1"), 10).unwrap(),
            Err(TicketError::BadSignature)
        );
    }

    #[test]
    fn invite_opens_only_for_requester() {
        let b = DeviceKeys::generate();
        let (id, bytes) = build_request(ticket("u1", &b, 1000));
        let req = parse_request(&bytes, &link("u1"), 10).unwrap().unwrap();
        let invite = Invite::generate(None);
        let msg = build_invite(&req, &invite);

        assert_eq!(open_invite(&msg, &id, &b), Some(*invite.secret()));
        assert_eq!(open_invite(&msg, &[0u8; 16], &b), None);
        assert_eq!(open_invite(&msg, &id, &DeviceKeys::generate()), None);
        // 요청은 초대로 읽히지 않는다
        assert_eq!(open_invite(&bytes, &id, &b), None);
    }

    #[test]
    fn non_link_payload_is_ignored() {
        assert!(parse_request(b"\x01\x02garbage", &link("u1"), 0).is_none());
    }
}
