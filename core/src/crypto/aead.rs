use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand_core::{OsRng, RngCore};

use super::group::ContentKey;
use super::CryptoError;

pub const NONCE_LEN: usize = 24;

/// 봉인 결과. `ciphertext`는 인증 태그(16바이트)를 포함한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

pub(super) fn seal_raw(
    key: &[u8; 32],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Sealed, CryptoError> {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    Ok(Sealed { nonce, ciphertext })
}

pub(super) fn open_raw(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)
}

/// `content_key`로 평문을 봉인한다. nonce는 매번 새로 뽑는다.
pub fn seal(key: &ContentKey, plaintext: &[u8], aad: &[u8]) -> Result<Sealed, CryptoError> {
    seal_raw(key.as_bytes(), plaintext, aad)
}

/// 봉인된 데이터를 연다. 변조·aad 불일치·다른 키·잘린 입력은 모두 `Decrypt` 에러.
pub fn open(
    key: &ContentKey,
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    open_raw(key.as_bytes(), nonce, ciphertext, aad)
}
