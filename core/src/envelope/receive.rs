use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use prost::Message;

use super::{envelope_aad, signature_input, EpochContext, MAX_TEXT_BYTES, VERSION};
use crate::crypto::{self, DeviceId, NONCE_LEN};
use crate::pb::{self, payload_body::Kind};

/// 중복 검사에 기억하는 `msg_id` 개수.
const DEDUP_CAPACITY: usize = 4096;
const WINDOW_MS: u64 = 10 * 60 * 1000;
const PUSH_PAST_WINDOW_MS: u64 = 60 * 60 * 1000;

/// 봉투가 들어온 경로. 푸시는 늦게 도착할 수 있어 과거 허용 폭이 넓다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Relay,
    Push,
}

/// 메시지를 버린 이유. 검증 순서(03-protocol 2.2)대로 나열한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DropReason {
    /// 봉투를 파싱할 수 없음
    Malformed,
    UnsupportedVersion,
    UnknownEpoch,
    Decrypt,
    /// 페이로드·본문 파싱 실패, 필드 길이 오류, 종류 없음
    BadBody,
    /// 텍스트가 최대 크기를 넘음
    TooLarge,
    EpochMismatch,
    UnknownSender,
    BadSignature,
    Duplicate,
    OutsideTimeWindow,
    FromSelf,
    /// 이전 에포크로 왔지만 지금은 그룹에 없는 기기가 보냈다 (제거된 기기)
    SenderRemoved,
}

/// 검증을 모두 통과한 메시지.
#[derive(Debug, Clone, PartialEq)]
pub struct Received {
    pub sender: DeviceId,
    pub msg_id: [u8; 16],
    pub created_at_ms: u64,
    pub epoch: u64,
    pub kind: Kind,
}

/// 받는 쪽 검증 파이프라인. 실패한 메시지는 조용히 버리고 카운터만 올린다.
pub struct Receiver {
    self_id: DeviceId,
    epochs: HashMap<u64, EpochContext>,
    seen: VecDeque<[u8; 16]>,
    seen_set: HashSet<[u8; 16]>,
    drops: BTreeMap<DropReason, u64>,
}

impl Receiver {
    pub fn new(self_id: DeviceId) -> Self {
        Self {
            self_id,
            epochs: HashMap::new(),
            seen: VecDeque::new(),
            seen_set: HashSet::new(),
            drops: BTreeMap::new(),
        }
    }

    /// 아는 에포크로 등록한다(현재 에포크와 유예 중인 이전 에포크).
    pub fn add_epoch(&mut self, ctx: EpochContext) {
        self.epochs.insert(ctx.epoch, ctx);
    }

    pub fn remove_epoch(&mut self, epoch: u64) {
        self.epochs.remove(&epoch);
    }

    /// 파이프라인 밖(앱 계층)에서 버린 메시지도 같은 카운터에 센다.
    pub(crate) fn count_drop(&mut self, reason: DropReason) {
        *self.drops.entry(reason).or_insert(0) += 1;
    }

    /// 이유별로 버린 메시지 수.
    pub fn drop_counts(&self) -> &BTreeMap<DropReason, u64> {
        &self.drops
    }

    /// 봉투 바이트를 검증하고 연다. 실패하면 이유를 돌려주고 카운터를 올린다.
    pub fn receive(
        &mut self,
        bytes: &[u8],
        source: Source,
        now_ms: u64,
    ) -> Result<Received, DropReason> {
        let result = self.verify(bytes, source, now_ms);
        if let Err(reason) = result {
            *self.drops.entry(reason).or_insert(0) += 1;
        }
        result
    }

    fn verify(
        &mut self,
        bytes: &[u8],
        source: Source,
        now_ms: u64,
    ) -> Result<Received, DropReason> {
        // 1. 버전, 아는 에포크
        let env = pb::SealedEnvelope::decode(bytes).map_err(|_| DropReason::Malformed)?;
        let nonce: [u8; NONCE_LEN] = env
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| DropReason::Malformed)?;
        if env.version != VERSION {
            return Err(DropReason::UnsupportedVersion);
        }
        let ctx = self
            .epochs
            .get(&env.epoch)
            .ok_or(DropReason::UnknownEpoch)?;

        // 2. AEAD 복호화
        let aad = envelope_aad(&ctx.channel_id, env.epoch);
        let plain = crypto::open(&ctx.content_key, &nonce, &env.ciphertext, &aad)
            .map_err(|_| DropReason::Decrypt)?;

        // 3. 본문 파싱, epoch 일치
        let signed =
            pb::SignedPayload::decode(plain.as_slice()).map_err(|_| DropReason::BadBody)?;
        let body =
            pb::PayloadBody::decode(signed.body.as_slice()).map_err(|_| DropReason::BadBody)?;
        let sender: [u8; 16] = body
            .sender_device_id
            .as_slice()
            .try_into()
            .map_err(|_| DropReason::BadBody)?;
        let msg_id: [u8; 16] = body
            .msg_id
            .as_slice()
            .try_into()
            .map_err(|_| DropReason::BadBody)?;
        let kind = body.kind.ok_or(DropReason::BadBody)?;
        if matches!(&kind, Kind::Text(t) if t.text.len() > MAX_TEXT_BYTES) {
            return Err(DropReason::TooLarge);
        }
        if body.epoch != env.epoch {
            return Err(DropReason::EpochMismatch);
        }

        // 4. 로스터에 있는 발신자
        let sender = DeviceId(sender);
        let sign_pub = ctx.roster.get(&sender).ok_or(DropReason::UnknownSender)?;

        // 5. 서명
        crypto::verify_signature(sign_pub, &signature_input(&signed.body), &signed.signature)
            .map_err(|_| DropReason::BadSignature)?;

        // 6. 중복
        if self.seen_set.contains(&msg_id) {
            return Err(DropReason::Duplicate);
        }

        // 7. 시간 창
        let past_limit = match source {
            Source::Relay => WINDOW_MS,
            Source::Push => PUSH_PAST_WINDOW_MS,
        };
        let too_old = now_ms.saturating_sub(body.created_at_ms) > past_limit;
        let too_new = body.created_at_ms.saturating_sub(now_ms) > WINDOW_MS;
        if too_old || too_new {
            return Err(DropReason::OutsideTimeWindow);
        }

        // 8. 자기 메시지
        if sender == self.self_id {
            return Err(DropReason::FromSelf);
        }

        self.remember(msg_id);
        Ok(Received {
            sender,
            msg_id,
            created_at_ms: body.created_at_ms,
            epoch: env.epoch,
            kind,
        })
    }

    #[cfg(test)]
    pub(super) fn has_seen(&self, msg_id: &[u8; 16]) -> bool {
        self.seen_set.contains(msg_id)
    }

    pub(super) fn remember(&mut self, msg_id: [u8; 16]) {
        if self.seen.len() == DEDUP_CAPACITY {
            if let Some(old) = self.seen.pop_front() {
                self.seen_set.remove(&old);
            }
        }
        self.seen.push_back(msg_id);
        self.seen_set.insert(msg_id);
    }
}
