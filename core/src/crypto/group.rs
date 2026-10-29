use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::device::DeviceId;

pub(super) fn hkdf_expand<const N: usize>(ikm: &[u8], salt: Option<&[u8]>, info: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    Hkdf::<Sha256>::new(salt, ikm)
        .expand(info, &mut out)
        .expect("출력 길이가 HKDF 한도 이내");
    out
}

fn info(label: &str, epoch: u64, extra: &[u8]) -> Vec<u8> {
    let mut v = label.as_bytes().to_vec();
    v.extend_from_slice(&epoch.to_le_bytes());
    v.extend_from_slice(extra);
    v
}

/// 에포크마다 새로 만드는 32바이트 그룹 비밀.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct GroupKey([u8; 32]);

impl GroupKey {
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        Self(k)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn content_key(&self, epoch: u64) -> ContentKey {
        ContentKey(hkdf_expand(
            &self.0,
            None,
            &info("clip-ferry v1 content", epoch, &[]),
        ))
    }

    pub fn channel_id(&self, epoch: u64) -> ChannelId {
        ChannelId(hkdf_expand(
            &self.0,
            None,
            &info("clip-ferry v1 channel", epoch, &[]),
        ))
    }

    pub fn device_tag(&self, epoch: u64, device: &DeviceId) -> DeviceTag {
        DeviceTag(hkdf_expand(
            &self.0,
            None,
            &info("clip-ferry v1 tag", epoch, &device.0),
        ))
    }
}

impl std::fmt::Debug for GroupKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GroupKey(<redacted>)")
    }
}

/// 메시지 본문 암호화 키.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ContentKey([u8; 32]);

impl ContentKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for ContentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContentKey(<redacted>)")
    }
}

/// 릴레이가 보는 채널 식별자. 능력(capability)으로 쓰이므로 `Debug`에 값을 찍지 않는다.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ChannelId(pub [u8; 32]);

impl std::fmt::Debug for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelId(<redacted>)")
    }
}

/// 릴레이가 보는 에포크별 기기 식별자.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DeviceTag(pub [u8; 16]);
