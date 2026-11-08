//! 계정 연결 참여 확인서 (F37, docs/03-protocol.md 4.4).
//!
//! 같은 계정의 기기들이 "이 공개키를 가진 새 기기는 이 계정에 로그인했다"를 확인할 수 있게
//! 접속 토큰과 같은 Ed25519 키로 서명한다. 서명하는 바이트는 코어 `link::ticket_message`와 같다.
//! 서버를 지나는 것은 공개키와 이름뿐이고 그룹 키는 없다.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::tokens::Signer;

pub const TICKET_DOMAIN: &[u8] = b"clip-ferry v1 link-ticket";
/// 확인서 유효 시간: 10분 (초대와 같다)
pub const TICKET_TTL_MS: i64 = 10 * 60 * 1000;

/// `"clip-ferry v1 link-ticket" ‖ u8 len ‖ user_id ‖ sign_pub ‖ dh_pub ‖ u8 len ‖ name ‖ u64_le expires_at_ms`
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

/// `POST /v1/link/ticket` 요청 본문. 키는 base64url(패딩 없음) 32바이트.
#[derive(Debug, Deserialize)]
pub struct TicketRequest {
    pub sign_pub: String,
    pub dh_pub: String,
    pub name: String,
}

/// 응답이자 앱이 `LinkTicket`으로 옮겨 싣는 값. 바이트는 base64url(패딩 없음).
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ticket {
    pub user_id: String,
    pub sign_pub: String,
    pub dh_pub: String,
    pub name: String,
    pub expires_at_ms: u64,
    pub kid: String,
    pub signature: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TicketError {
    BadKey,
    BadName,
}

fn key32(text: &str) -> Result<[u8; 32], TicketError> {
    URL_SAFE_NO_PAD
        .decode(text.trim().trim_end_matches('='))
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(TicketError::BadKey)
}

pub fn issue(
    signer: &Signer,
    user_id: &str,
    req: &TicketRequest,
    now_ms: i64,
) -> Result<Ticket, TicketError> {
    let sign_pub = key32(&req.sign_pub)?;
    let dh_pub = key32(&req.dh_pub)?;
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(TicketError::BadName);
    }
    let expires_at_ms = (now_ms + TICKET_TTL_MS) as u64;
    let msg = ticket_message(user_id, &sign_pub, &dh_pub, name, expires_at_ms)
        .ok_or(TicketError::BadName)?;
    Ok(Ticket {
        user_id: user_id.to_string(),
        sign_pub: URL_SAFE_NO_PAD.encode(sign_pub),
        dh_pub: URL_SAFE_NO_PAD.encode(dh_pub),
        name: name.to_string(),
        expires_at_ms,
        kid: signer.kid().to_string(),
        signature: URL_SAFE_NO_PAD.encode(signer.sign_raw(&msg)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_message_matches_fixed_vector() {
        // 코어(core/src/link.rs)의 같은 이름 테스트와 같은 값이어야 한다.
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
    fn issued_ticket_verifies_with_public_key() {
        use ed25519_dalek::{Signature, VerifyingKey};
        let signer = Signer::from_seed(&[3u8; 32]);
        let req = TicketRequest {
            sign_pub: URL_SAFE_NO_PAD.encode([5u8; 32]),
            dh_pub: URL_SAFE_NO_PAD.encode([6u8; 32]),
            name: " 새 폰 ".into(),
        };
        let t = issue(&signer, "user-1", &req, 1_000).unwrap();
        assert_eq!(t.name, "새 폰");
        assert_eq!(t.expires_at_ms, 1_000 + TICKET_TTL_MS as u64);
        let msg =
            ticket_message("user-1", &[5u8; 32], &[6u8; 32], "새 폰", t.expires_at_ms).unwrap();
        let sig: [u8; 64] = URL_SAFE_NO_PAD
            .decode(&t.signature)
            .unwrap()
            .try_into()
            .unwrap();
        VerifyingKey::from_bytes(&signer.public())
            .unwrap()
            .verify_strict(&msg, &Signature::from_bytes(&sig))
            .unwrap();
    }

    #[test]
    fn rejects_bad_input() {
        let signer = Signer::from_seed(&[3u8; 32]);
        let mut req = TicketRequest {
            sign_pub: "short".into(),
            dh_pub: URL_SAFE_NO_PAD.encode([6u8; 32]),
            name: "x".into(),
        };
        assert_eq!(issue(&signer, "u", &req, 0), Err(TicketError::BadKey));
        req.sign_pub = URL_SAFE_NO_PAD.encode([5u8; 32]);
        req.name = "  ".into();
        assert_eq!(issue(&signer, "u", &req, 0), Err(TicketError::BadName));
        req.name = "가".repeat(65);
        assert_eq!(issue(&signer, "u", &req, 0), Err(TicketError::BadName));
    }
}
