//! 암호 기본: 기기 키, 그룹 키, 파생값, 봉인/개봉, 기기 간 래핑.
//! 규격은 docs/03-protocol.md 1절.

mod aead;
mod device;
mod group;
mod wrap;

#[cfg(test)]
mod tests;

pub use aead::{open, seal, Sealed, NONCE_LEN};
pub use device::{DeviceId, DeviceKeys};
pub use group::{ChannelId, ContentKey, DeviceTag, GroupKey};
pub use wrap::{unwrap, wrap_for, Wrapped};

/// 암호 연산 실패. 원인을 구분해 알려주지 않는다.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("복호화 또는 인증 실패")]
    Decrypt,
    #[error("입력 길이가 올바르지 않음")]
    InvalidLength,
}
