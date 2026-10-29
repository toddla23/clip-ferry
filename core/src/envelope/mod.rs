//! 봉투: 페이로드 서명·봉인과 수신 검증 파이프라인 (docs/03-protocol.md 2절).

mod receive;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use prost::Message;
use rand_core::{OsRng, RngCore};

use crate::crypto::{self, ChannelId, ContentKey, CryptoError, DeviceId, DeviceKeys, GroupKey};
use crate::pb::{self, payload_body::Kind};

pub use receive::{DropReason, Received, Receiver, Source};

/// 봉투 버전.
pub const VERSION: u32 = 1;

const SIG_DOMAIN: &[u8] = b"clip-ferry v1 sig";
const AAD_PREFIX: &[u8] = b"clip-ferry/v1";

#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    #[error(transparent)]
    Crypto(#[from] CryptoError),
}

/// 한 에포크에서 봉투를 만들고 여는 데 필요한 값.
pub struct EpochContext {
    epoch: u64,
    content_key: ContentKey,
    channel_id: ChannelId,
    roster: HashMap<DeviceId, [u8; 32]>,
}

impl EpochContext {
    /// `sign_pubs`는 이 에포크 로스터에 있는 모든 기기의 서명 공개키.
    pub fn new(
        group: &GroupKey,
        epoch: u64,
        sign_pubs: impl IntoIterator<Item = [u8; 32]>,
    ) -> Self {
        Self {
            epoch,
            content_key: group.content_key(epoch),
            channel_id: group.channel_id(epoch),
            roster: sign_pubs
                .into_iter()
                .map(|p| (DeviceId::from_sign_pub(&p), p))
                .collect(),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn channel_id(&self) -> &ChannelId {
        &self.channel_id
    }
}

/// aad = "clip-ferry/v1" ‖ channel_id ‖ epoch(LE)
fn envelope_aad(channel_id: &ChannelId, epoch: u64) -> Vec<u8> {
    let mut aad = AAD_PREFIX.to_vec();
    aad.extend_from_slice(&channel_id.0);
    aad.extend_from_slice(&epoch.to_le_bytes());
    aad
}

fn signature_input(body: &[u8]) -> Vec<u8> {
    let mut m = SIG_DOMAIN.to_vec();
    m.extend_from_slice(body);
    m
}

/// 보낼 봉투. `bytes`가 `Publish.payload`가 된다.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub msg_id: [u8; 16],
    pub bytes: Vec<u8>,
}

/// `kind`를 서명하고 봉인한다.
pub fn seal_message(
    keys: &DeviceKeys,
    ctx: &EpochContext,
    kind: Kind,
    now_ms: u64,
) -> Result<Outgoing, EnvelopeError> {
    let mut msg_id = [0u8; 16];
    OsRng.fill_bytes(&mut msg_id);
    let body = pb::PayloadBody {
        sender_device_id: keys.device_id().0.to_vec(),
        msg_id: msg_id.to_vec(),
        created_at_ms: now_ms,
        epoch: ctx.epoch,
        kind: Some(kind),
    };
    let bytes = build(keys, ctx, VERSION, &body)?;
    Ok(Outgoing { msg_id, bytes })
}

/// 임의의 본문·버전으로 봉투를 만든다. 테스트가 잘못된 봉투를 만들 때도 쓴다.
fn build(
    signer: &DeviceKeys,
    ctx: &EpochContext,
    version: u32,
    body: &pb::PayloadBody,
) -> Result<Vec<u8>, EnvelopeError> {
    let body = body.encode_to_vec();
    let signed = pb::SignedPayload {
        signature: signer.sign(&signature_input(&body)).to_vec(),
        body,
    };
    let aad = envelope_aad(&ctx.channel_id, ctx.epoch);
    let sealed = crypto::seal(&ctx.content_key, &signed.encode_to_vec(), &aad)?;
    Ok(pb::SealedEnvelope {
        version,
        epoch: ctx.epoch,
        nonce: sealed.nonce.to_vec(),
        ciphertext: sealed.ciphertext,
    }
    .encode_to_vec())
}
