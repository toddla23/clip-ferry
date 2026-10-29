use rand_core::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey};
use zeroize::Zeroizing;

use super::aead::{open_raw, seal_raw, NONCE_LEN};
use super::device::{DeviceId, DeviceKeys};
use super::group::hkdf_expand;
use super::CryptoError;

const WRAP_INFO: &[u8] = b"clip-ferry v1 wrap";

/// 특정 기기만 열 수 있게 래핑된 데이터.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub eph_pub: [u8; 32],
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

fn wrap_key(
    shared: &[u8; 32],
    eph_pub: &[u8; 32],
    recipient_dh_pub: &[u8; 32],
) -> Zeroizing<[u8; 32]> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(eph_pub);
    salt[32..].copy_from_slice(recipient_dh_pub);
    Zeroizing::new(hkdf_expand(shared, Some(&salt), WRAP_INFO))
}

/// 수신 기기의 `dh_pub`로 `plaintext`를 래핑한다. aad는 수신 기기의 `device_id`.
pub fn wrap_for(
    recipient_dh_pub: &[u8; 32],
    recipient_id: &DeviceId,
    plaintext: &[u8],
) -> Result<Wrapped, CryptoError> {
    let eph = EphemeralSecret::random_from_rng(OsRng);
    let eph_pub = PublicKey::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&PublicKey::from(*recipient_dh_pub));
    let k = wrap_key(shared.as_bytes(), &eph_pub, recipient_dh_pub);
    let sealed = seal_raw(&k, plaintext, &recipient_id.0)?;
    Ok(Wrapped {
        eph_pub,
        nonce: sealed.nonce,
        ciphertext: sealed.ciphertext,
    })
}

/// 자신에게 래핑된 데이터를 연다.
pub fn unwrap(keys: &DeviceKeys, wrapped: &Wrapped) -> Result<Vec<u8>, CryptoError> {
    let shared = keys
        .dh_secret()
        .diffie_hellman(&PublicKey::from(wrapped.eph_pub));
    let k = wrap_key(shared.as_bytes(), &wrapped.eph_pub, &keys.dh_public());
    open_raw(&k, &wrapped.nonce, &wrapped.ciphertext, &keys.device_id().0)
}
