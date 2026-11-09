//! 접속 토큰(JWT, EdDSA)과 갱신 토큰.
//!
//! - 접속 토큰: 1시간. 릴레이(F33)는 `GET /v1/keys`의 공개키로 서명만 확인한다.
//! - 갱신 토큰: 무작위 32바이트(base64url). 서버는 SHA-256 해시만 저장하고, 쓸 때마다 새것으로 바꾼다.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::SigningKey;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const ISSUER: &str = "clip-ferry-account";
/// 접속 토큰을 받는 쪽(릴레이)
pub const AUDIENCE: &str = "clip-ferry-relay";
pub const ACCESS_TTL_SECS: i64 = 60 * 60;
pub const REFRESH_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessClaims {
    pub iss: String,
    pub aud: String,
    /// 계정 ID
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("서명 키는 base64url 32바이트여야 합니다")]
    BadKey,
    #[error("접속 토큰이 올바르지 않습니다")]
    Invalid,
}

/// 접속 토큰 서명 키. 시드(32바이트)는 환경 변수로만 받는다.
pub struct Signer {
    kid: String,
    public: [u8; 32],
    /// 계정 연결 참여 확인서(F37) 서명용
    signing: SigningKey,
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl Signer {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(seed);
        let public = signing.verifying_key().to_bytes();
        // PKCS#8 v1 (RFC 8410) 꼴: 고정 머리말 16바이트 + 시드 32바이트
        let mut der = Zeroizing::new(Vec::with_capacity(48));
        der.extend_from_slice(&[
            0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
            0x04, 0x20,
        ]);
        der.extend_from_slice(seed);
        let kid = hex::encode(&Sha256::digest(public)[..8]);
        Signer {
            kid,
            public,
            signing,
            encoding: EncodingKey::from_ed_der(&der),
            decoding: DecodingKey::from_ed_components(&URL_SAFE_NO_PAD.encode(public))
                .expect("올바른 공개키"),
        }
    }

    /// 환경 변수 값(base64url, 패딩 있어도 됨)에서 키를 읽는다.
    pub fn from_base64(text: &str) -> Result<Self, TokenError> {
        let trimmed = text.trim().trim_end_matches('=');
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(trimmed)
                .map_err(|_| TokenError::BadKey)?,
        );
        let seed: Zeroizing<[u8; 32]> = Zeroizing::new(
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| TokenError::BadKey)?,
        );
        Ok(Self::from_seed(&seed))
    }

    /// 새 시드를 만들어 base64url로 돌려준다 (`gen-key` 명령).
    pub fn generate_seed_base64() -> String {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::getrandom(seed.as_mut()).expect("OS 난수를 얻지 못했습니다");
        URL_SAFE_NO_PAD.encode(seed.as_ref())
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    /// 임의 바이트에 서명한다 (계정 연결 참여 확인서, `link`).
    pub fn sign_raw(&self, msg: &[u8]) -> [u8; 64] {
        use ed25519_dalek::Signer as _;
        self.signing.sign(msg).to_bytes()
    }

    /// 계정 `user_id`의 접속 토큰. 돌려주는 두 번째 값은 만료 시각(Unix 초).
    pub fn issue(&self, user_id: &str, now_secs: i64) -> (String, i64) {
        let exp = now_secs + ACCESS_TTL_SECS;
        let claims = AccessClaims {
            iss: ISSUER.into(),
            aud: AUDIENCE.into(),
            sub: user_id.into(),
            iat: now_secs,
            exp,
        };
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.kid.clone());
        let token = jsonwebtoken::encode(&header, &claims, &self.encoding).expect("서명 실패");
        (token, exp)
    }

    /// 접속 토큰 확인 (서명·발급자·대상·만료).
    pub fn verify(&self, token: &str) -> Result<AccessClaims, TokenError> {
        let mut v = Validation::new(Algorithm::EdDSA);
        v.set_issuer(&[ISSUER]);
        v.set_audience(&[AUDIENCE]);
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        v.leeway = 30;
        jsonwebtoken::decode::<AccessClaims>(token, &self.decoding, &v)
            .map(|d| d.claims)
            .map_err(|_| TokenError::Invalid)
    }

    /// `GET /v1/keys` 응답 (JWKS 꼴, RFC 8037).
    pub fn jwks(&self) -> serde_json::Value {
        serde_json::json!({
            "keys": [{
                "kty": "OKP",
                "crv": "Ed25519",
                "alg": "EdDSA",
                "use": "sig",
                "kid": self.kid,
                "x": URL_SAFE_NO_PAD.encode(self.public),
            }]
        })
    }
}

/// 새 갱신 토큰과 그 해시.
pub fn new_refresh_token() -> (String, [u8; 32]) {
    let mut raw = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(raw.as_mut()).expect("OS 난수를 얻지 못했습니다");
    let token = URL_SAFE_NO_PAD.encode(raw.as_ref());
    let hash = refresh_hash(&token);
    (token, hash)
}

/// 저장·조회에 쓰는 갱신 토큰 해시.
pub fn refresh_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> Signer {
        Signer::from_seed(&[7u8; 32])
    }

    #[test]
    fn issue_then_verify() {
        let s = signer();
        let now = crate::now_ms() / 1000;
        let (t, exp) = s.issue("abc", now);
        assert_eq!(exp, now + ACCESS_TTL_SECS);
        let c = s.verify(&t).unwrap();
        assert_eq!(c.sub, "abc");
        assert_eq!(c.aud, AUDIENCE);
    }

    #[test]
    fn other_key_rejected() {
        let now = crate::now_ms() / 1000;
        let (t, _) = signer().issue("abc", now);
        assert!(Signer::from_seed(&[8u8; 32]).verify(&t).is_err());
    }

    #[test]
    fn expired_rejected() {
        let s = signer();
        let (t, _) = s.issue("abc", crate::now_ms() / 1000 - ACCESS_TTL_SECS - 120);
        assert!(s.verify(&t).is_err());
    }

    #[test]
    fn tampered_rejected() {
        let s = signer();
        let (t, _) = s.issue("abc", crate::now_ms() / 1000);
        let mut parts: Vec<String> = t.split('.').map(String::from).collect();
        let forged = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "iss": ISSUER, "aud": AUDIENCE, "sub": "someone-else",
                "iat": 0, "exp": 9_999_999_999i64
            }))
            .unwrap(),
        );
        parts[1] = forged;
        assert!(s.verify(&parts.join(".")).is_err());
    }

    #[test]
    fn key_from_base64_round_trip() {
        let text = Signer::generate_seed_base64();
        let a = Signer::from_base64(&text).unwrap();
        let b = Signer::from_base64(&text).unwrap();
        assert_eq!(a.kid(), b.kid());
        assert!(Signer::from_base64("short").is_err());
    }

    #[test]
    fn refresh_tokens_differ_and_hash_matches() {
        let (a, ha) = new_refresh_token();
        let (b, _) = new_refresh_token();
        assert_ne!(a, b);
        assert_eq!(refresh_hash(&a), ha);
        assert_ne!(ha.as_slice(), a.as_bytes());
    }
}
