use rand_core::{OsRng, RngCore};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::aead::{open_raw, seal_raw, Sealed, NONCE_LEN};
use super::CryptoError;

/// 로컬 상태 파일(`state.bin`)을 암호화하는 키. 기기마다 한 번 만들어 비밀 저장소에 둔다.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct StateKey([u8; 32]);

impl StateKey {
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        Self(k)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let k: [u8; 32] = bytes.try_into().map_err(|_| CryptoError::InvalidLength)?;
        Ok(Self(k))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<Sealed, CryptoError> {
        seal_raw(&self.0, plaintext, aad)
    }

    pub fn open(
        &self,
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        open_raw(&self.0, nonce, ciphertext, aad)
    }
}

impl std::fmt::Debug for StateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StateKey(<redacted>)")
    }
}
