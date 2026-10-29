use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use super::CryptoError;

/// `SHA-256(sign_pub)[0..16]`
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DeviceId(pub [u8; 16]);

impl DeviceId {
    pub fn from_sign_pub(sign_pub: &[u8; 32]) -> Self {
        let digest = Sha256::digest(sign_pub);
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        Self(id)
    }

    /// 화면·로그용 짧은 표기: 앞 4바이트 hex.
    pub fn short_hex(&self) -> String {
        self.0[..4].iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// 서명 검증. 공개키가 잘못됐거나 서명이 맞지 않으면 `Signature` 에러.
pub fn verify_signature(sign_pub: &[u8; 32], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
    let key = VerifyingKey::from_bytes(sign_pub).map_err(|_| CryptoError::Signature)?;
    let sig: &[u8; 64] = sig.try_into().map_err(|_| CryptoError::Signature)?;
    key.verify_strict(msg, &ed25519_dalek::Signature::from_bytes(sig))
        .map_err(|_| CryptoError::Signature)
}

/// 기기 키 쌍: 서명용 Ed25519 + 키 교환용 X25519. 드롭 시 비밀이 지워진다.
pub struct DeviceKeys {
    sign: SigningKey,
    dh: StaticSecret,
}

impl DeviceKeys {
    pub fn generate() -> Self {
        Self {
            sign: SigningKey::generate(&mut OsRng),
            dh: StaticSecret::random_from_rng(OsRng),
        }
    }

    /// 직렬화 형식: `sign_seed(32) ‖ dh_secret(32)`.
    pub fn to_bytes(&self) -> Zeroizing<[u8; 64]> {
        let mut out = Zeroizing::new([0u8; 64]);
        out[..32].copy_from_slice(&self.sign.to_bytes());
        out[32..].copy_from_slice(&self.dh.to_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let bytes: &[u8; 64] = bytes.try_into().map_err(|_| CryptoError::InvalidLength)?;
        let mut seed = Zeroizing::new([0u8; 32]);
        let mut dh = Zeroizing::new([0u8; 32]);
        seed.copy_from_slice(&bytes[..32]);
        dh.copy_from_slice(&bytes[32..]);
        Ok(Self {
            sign: SigningKey::from_bytes(&seed),
            dh: StaticSecret::from(*dh),
        })
    }

    pub fn sign_public(&self) -> [u8; 32] {
        self.sign.verifying_key().to_bytes()
    }

    pub fn dh_public(&self) -> [u8; 32] {
        PublicKey::from(&self.dh).to_bytes()
    }

    /// Ed25519 서명 (64바이트).
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.sign.sign(msg).to_bytes()
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_sign_pub(&self.sign_public())
    }

    pub(super) fn dh_secret(&self) -> &StaticSecret {
        &self.dh
    }
}

impl std::fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceKeys")
            .field("device_id", &self.device_id())
            .field("secrets", &"<redacted>")
            .finish()
    }
}
