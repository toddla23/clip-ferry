//! 셸이 쓰는 공개 API.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use zeroize::Zeroizing;

use crate::crypto::{ChannelId, CryptoError, DeviceId, DeviceKeys, DeviceTag, GroupKey, StateKey};
use crate::envelope::{
    self, DropReason, EnvelopeError, EpochContext, Receiver, Source, MAX_TEXT_BYTES,
};
use crate::group;
use crate::pairing::{
    self, Identity, Invite, Inviter, JoinedGroup, Joiner, JoinerInfo, PairingError,
};
use crate::pb;
use crate::store::{self, SecretStore, StateFileError, StoreError};
use crate::transfer::{self, Assembler, ImageError};

const KEY_DEVICE: &str = "device_keys";
const KEY_STATE: &str = "state_key";

fn group_key_name(epoch: u64) -> String {
    format!("group_key/{epoch}")
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 오류 메시지에는 키·내용·이름 같은 값을 넣지 않는다.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("이미 초기화된 기기입니다")]
    AlreadyInitialized,
    #[error("먼저 기기를 초기화해야 합니다")]
    NotInitialized,
    #[error("이미 그룹이 있습니다")]
    GroupExists,
    #[error("그룹이 없습니다")]
    NoGroup,
    #[error(transparent)]
    Pairing(#[from] PairingError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
    #[error("받은 그룹 정보가 이 기기와 맞지 않습니다")]
    BadGroup,
    #[error("removed or stale: 이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요")]
    Removed,
    #[error("기기 이름은 1~64바이트여야 합니다")]
    BadName,
    #[error("그런 기기가 없습니다")]
    UnknownDevice,
    #[error("이름이나 id 앞자리가 여러 기기와 겹칩니다. 더 길게 쓰세요")]
    AmbiguousDevice,
    #[error("자기 자신은 제거할 수 없습니다")]
    CannotRemoveSelf,
    #[error("보낼 텍스트가 비어 있습니다")]
    EmptyText,
    #[error("텍스트가 너무 큽니다 (최대 1 MiB)")]
    TextTooLarge,
    #[error("보낼 이미지가 비어 있습니다")]
    EmptyImage,
    #[error("이미지가 너무 큽니다 (최대 20 MiB)")]
    ImageTooLarge,
    #[error("지원하지 않는 이미지 형식입니다 (PNG, JPEG, GIF, WebP만 가능)")]
    UnsupportedImage,
    #[error("로컬 상태 파일을 열 수 없습니다. 손상되었거나 변조되었을 수 있습니다")]
    StateCorrupt,
    #[error("로컬 상태와 비밀 저장소가 서로 맞지 않습니다. 한쪽만 남아 있습니다")]
    StateInconsistent,
    #[error(transparent)]
    Secret(#[from] StoreError),
    #[error("로컬 상태 파일 입출력 오류")]
    Io,
}

impl From<StateFileError> for Error {
    fn from(e: StateFileError) -> Self {
        match e {
            StateFileError::Io => Error::Io,
            StateFileError::Corrupt => Error::StateCorrupt,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// `state.bin`을 둘 폴더.
    pub data_dir: PathBuf,
}

/// 화면에 보여도 되는 요약. 비밀값은 없다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub device_name: String,
    pub device_id: DeviceId,
    /// 그룹이 있을 때만.
    pub epoch: Option<u64>,
    pub member_count: usize,
    pub health: GroupHealth,
}

/// 그룹과의 관계.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupHealth {
    Active,
    /// 그룹에서 제거되었다. 다시 페어링해야 한다.
    Removed,
    /// 30일 넘게 다른 기기와 연락이 없다. 제거되었거나 너무 오래 연결되지 않은 것이다.
    Stale,
}

/// 구독해야 하는 채널 하나 (현재 에포크와 유예 중인 이전 에포크).
#[derive(Debug, Clone)]
pub struct Subscription {
    pub epoch: u64,
    pub channel_id: ChannelId,
    pub device_tag: DeviceTag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub device_id: DeviceId,
    pub name: String,
    pub is_self: bool,
    pub added_at_ms: u64,
}

/// `approve_join`이 돌려주는, 릴레이에 게시할 메시지들.
#[derive(Debug)]
pub struct Approval {
    /// `pair_channel`에 게시: 새 기기에게 가는 환영 메시지
    pub welcome: Vec<u8>,
    pub pair_channel: [u8; 32],
    /// `group_channel`에 게시: 기존 구성원에게 가는 `RosterUpdate`
    pub roster_update: Vec<u8>,
    pub group_channel: ChannelId,
    pub joiner: JoinerInfo,
}

/// 봉인된 메시지. `bytes`를 `channel_id` 채널에 게시한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedMessage {
    pub channel_id: ChannelId,
    pub msg_id: [u8; 16],
    pub bytes: Vec<u8>,
}

/// 릴레이에 접속할 때 쓰는 값.
#[derive(Debug, Clone)]
pub struct Connection {
    pub epoch: u64,
    pub channel_id: ChannelId,
    pub device_tag: DeviceTag,
}

/// 받은 텍스트.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub from: DeviceId,
    pub from_name: String,
    pub text: String,
    pub created_at_ms: u64,
}

/// 받은 이미지. 크기와 해시를 확인한 원본 바이트다(디코딩은 셸이 한다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageClip {
    pub from: DeviceId,
    pub from_name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    /// 보낸 쪽이 알린 크기. 믿지 말고 디코딩해서 확인한다.
    pub width: u32,
    pub height: u32,
    pub created_at_ms: u64,
}

/// `handle_deliver`가 받은 메시지로 한 일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Clip(Clip),
    /// 이미지가 다 모였다.
    Image(ImageClip),
    /// 로스터가 바뀌었다 (기기 목록을 다시 그린다).
    DevicesChanged,
    /// 키가 회전되어 새 에포크로 넘어갔다 (새 채널을 구독해야 한다).
    EpochChanged(u64),
    /// 이 기기가 그룹에서 제거된 것을 알았다.
    Removed,
    /// 돌아온 기기의 `EpochQuery`에 대한 응답. 각 메시지를 해당 채널에 게시한다.
    Replies(Vec<SealedMessage>),
    /// 처리할 것이 없는 메시지 (아직 다루지 않는 종류 등).
    Nothing,
}

struct Device {
    keys: DeviceKeys,
    state_key: StateKey,
    state: pb::LocalState,
    group_key: Option<GroupKey>,
    /// 유예 중인 이전 에포크의 그룹 키 (`state.retired`와 짝)
    retired_keys: HashMap<u64, GroupKey>,
    receiver: Option<Receiver>,
    last_contact_ms: u64,
    /// `state.bin`에 마지막으로 적은 `last_contact_ms`
    persisted_contact_ms: u64,
    removed: bool,
    /// 받는 중인 이미지 (메모리에만 둔다)
    assembler: Assembler,
}

impl Device {
    /// 현재 로스터로 수신 검증 파이프라인을 맞춘다 (중복 목록은 유지).
    fn sync_receiver(&mut self) {
        let (Some(group_key), Some(roster)) = (&self.group_key, &self.state.roster) else {
            return;
        };
        let mut contexts = vec![EpochContext::new(
            group_key,
            roster.epoch,
            roster_sign_pubs(roster),
        )];
        for retired in &self.state.retired {
            if let (Some(key), Some(old)) = (self.retired_keys.get(&retired.epoch), &retired.roster)
            {
                contexts.push(EpochContext::new(key, retired.epoch, roster_sign_pubs(old)));
            }
        }
        let receiver = self
            .receiver
            .get_or_insert_with(|| Receiver::new(self.keys.device_id()));
        for ctx in contexts {
            receiver.add_epoch(ctx);
        }
    }

    fn send_context(&self) -> Result<EpochContext, Error> {
        let (Some(group_key), Some(roster)) = (&self.group_key, &self.state.roster) else {
            return Err(Error::NoGroup);
        };
        Ok(EpochContext::new(
            group_key,
            roster.epoch,
            roster_sign_pubs(roster),
        ))
    }
}

fn roster_sign_pubs(roster: &pb::Roster) -> Vec<[u8; 32]> {
    roster
        .members
        .iter()
        .filter_map(|m| <[u8; 32]>::try_from(m.sign_pub.as_slice()).ok())
        .collect()
}

/// 로스터의 각 구성원이 `device_id == SHA-256(sign_pub)[0..16]`를 만족하는지.
fn roster_is_consistent(roster: &pb::Roster) -> bool {
    roster.members.iter().all(|m| {
        <[u8; 32]>::try_from(m.sign_pub.as_slice())
            .is_ok_and(|p| DeviceId::from_sign_pub(&p).0.as_slice() == m.device_id.as_slice())
            && m.dh_pub.len() == 32
    })
}

/// 동시에 수정된 로스터 중 어느 쪽을 채택할지: `version`이 크면, 같으면 직렬화 해시가 큰 쪽.
fn roster_wins(new: &pb::Roster, current: &pb::Roster) -> bool {
    use prost::Message;
    use sha2::{Digest, Sha256};
    if new.version != current.version {
        return new.version > current.version;
    }
    Sha256::digest(new.encode_to_vec()).as_slice()
        > Sha256::digest(current.encode_to_vec()).as_slice()
}

pub struct ClipFerry {
    config: Config,
    secrets: Box<dyn SecretStore>,
    device: Mutex<Option<Device>>,
}

impl ClipFerry {
    /// 저장된 기기가 있으면 불러온다. 없으면 초기화 전 상태로 열린다.
    pub fn open(config: Config, secrets: Box<dyn SecretStore>) -> Result<Self, Error> {
        let keys = secrets.get(KEY_DEVICE).map(Zeroizing::new);
        let state_key = secrets.get(KEY_STATE).map(Zeroizing::new);
        let path = config.data_dir.join("state.bin");

        let device = match (keys, state_key) {
            (None, None) => {
                if path.exists() {
                    return Err(Error::StateInconsistent);
                }
                None
            }
            (Some(keys), Some(state_key)) => {
                let keys = DeviceKeys::from_bytes(&keys).map_err(|_| Error::StateCorrupt)?;
                let state_key =
                    StateKey::from_bytes(&state_key).map_err(|_| Error::StateCorrupt)?;
                let state =
                    store::read_state(&path, &state_key)?.ok_or(Error::StateInconsistent)?;
                let group_key = match &state.roster {
                    Some(roster) => Some(load_group_key(secrets.as_ref(), roster.epoch)?),
                    None => None,
                };
                let mut retired_keys = HashMap::new();
                for r in &state.retired {
                    if let Ok(key) = load_group_key(secrets.as_ref(), r.epoch) {
                        retired_keys.insert(r.epoch, key);
                    }
                }
                let persisted = state.last_contact_ms;
                let removed = state.status == 1;
                let mut dev = Device {
                    keys,
                    state_key,
                    state,
                    group_key,
                    retired_keys,
                    receiver: None,
                    // 예전 버전 상태 파일에는 값이 없다. 지금부터 센다.
                    last_contact_ms: if persisted == 0 { now_ms() } else { persisted },
                    persisted_contact_ms: persisted,
                    removed,
                    assembler: Assembler::default(),
                };
                dev.sync_receiver();
                Some(dev)
            }
            _ => return Err(Error::StateInconsistent),
        };

        Ok(Self {
            config,
            secrets,
            device: Mutex::new(device),
        })
    }

    pub fn is_initialized(&self) -> bool {
        self.device.lock().unwrap().is_some()
    }

    /// 기기 키와 로컬 저장 키를 만들고 이름을 정한다.
    pub fn init_device(&self, device_name: &str) -> Result<(), Error> {
        let mut slot = self.device.lock().unwrap();
        if slot.is_some() {
            return Err(Error::AlreadyInitialized);
        }
        let keys = DeviceKeys::generate();
        let state_key = StateKey::generate();
        let state = pb::LocalState {
            version: 1,
            device_name: device_name.to_string(),
            roster: None,
            created_at_ms: now_ms(),
            retired: Vec::new(),
            last_contact_ms: 0,
            status: 0,
        };

        self.secrets.set(KEY_DEVICE, &keys.to_bytes()[..])?;
        self.secrets.set(KEY_STATE, state_key.as_bytes())?;
        if let Err(e) = store::write_state(&self.state_path(), &state_key, &state) {
            // 반쯤 만들어진 채로 남지 않게 되돌린다.
            let _ = self.secrets.delete(KEY_DEVICE);
            let _ = self.secrets.delete(KEY_STATE);
            return Err(e.into());
        }
        *slot = Some(Device {
            keys,
            state_key,
            state,
            group_key: None,
            retired_keys: HashMap::new(),
            receiver: None,
            last_contact_ms: now_ms(),
            persisted_contact_ms: 0,
            removed: false,
            assembler: Assembler::default(),
        });
        Ok(())
    }

    /// 에포크 1의 새 그룹을 만든다. 구성원은 이 기기뿐이다.
    pub fn create_group(&self) -> Result<(), Error> {
        let mut slot = self.device.lock().unwrap();
        let dev = slot.as_mut().ok_or(Error::NotInitialized)?;
        if dev.state.roster.is_some() {
            return Err(Error::GroupExists);
        }
        let (group_key, roster) = group::create(&dev.keys, &dev.state.device_name, now_ms());
        self.install_group(dev, group_key, roster)
    }

    /// 그룹 키와 로스터를 저장하고 현재 그룹으로 삼는다. 저장에 실패하면 비밀도 되돌린다.
    fn install_group(
        &self,
        dev: &mut Device,
        group_key: GroupKey,
        roster: pb::Roster,
    ) -> Result<(), Error> {
        let mut state = dev.state.clone();
        let epoch = roster.epoch;
        state.roster = Some(roster);
        state.last_contact_ms = now_ms();
        state.status = 0;

        self.secrets
            .set(&group_key_name(epoch), group_key.as_bytes())?;
        if let Err(e) = store::write_state(&self.state_path(), &dev.state_key, &state) {
            let _ = self.secrets.delete(&group_key_name(epoch));
            return Err(e.into());
        }
        dev.last_contact_ms = state.last_contact_ms;
        dev.persisted_contact_ms = state.last_contact_ms;
        dev.removed = false;
        dev.state = state;
        dev.group_key = Some(group_key);
        dev.sync_receiver();
        Ok(())
    }

    /// 새 기기를 초대한다. 돌려받은 `Inviter`로 릴레이 메시지를 처리한다.
    pub fn create_invite(&self, relay: Option<String>) -> Result<Inviter, Error> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        if dev.state.roster.is_none() {
            return Err(Error::NoGroup);
        }
        Ok(Inviter::new(
            Invite::generate(relay),
            Identity::of(&dev.keys, &dev.state.device_name),
            now_ms(),
        ))
    }

    /// 사용자가 SAS를 확인하고 승인했다. 로스터에 새 기기를 넣어 저장하고 보낼 메시지를 돌려준다.
    ///
    /// 환영 메시지가 상대에게 닿지 못해도 로스터에는 이미 들어가 있다. 그때는 기기를 제거하면 된다.
    pub fn approve_join(&self, inviter: &mut Inviter, now_ms: u64) -> Result<Approval, Error> {
        let mut slot = self.device.lock().unwrap();
        let dev = slot.as_mut().ok_or(Error::NotInitialized)?;
        let roster = dev.state.roster.clone().ok_or(Error::NoGroup)?;
        let group_key = dev.group_key.clone().ok_or(Error::NoGroup)?;

        let pending = inviter.pending().ok_or(PairingError::NotPending)?;
        if roster
            .members
            .iter()
            .any(|m| m.device_id == pending.device_id().0)
        {
            return Err(PairingError::AlreadyMember.into());
        }
        let joiner = inviter.take_approved(now_ms)?;

        let mut new_roster = roster;
        new_roster.version += 1;
        new_roster.members.push(pb::Member {
            device_id: joiner.device_id().0.to_vec(),
            sign_pub: joiner.sign_pub.to_vec(),
            dh_pub: joiner.dh_pub.to_vec(),
            name: joiner.name.clone(),
            added_at_ms: now_ms,
            push: None,
        });
        let welcome = pairing::build_welcome(&dev.keys, &joiner, &group_key, &new_roster)?;

        let mut state = dev.state.clone();
        state.roster = Some(new_roster.clone());
        store::write_state(&self.state_path(), &dev.state_key, &state)?;
        dev.state = state;
        dev.sync_receiver();

        // 그룹의 다른 기기들에게 알릴 `RosterUpdate`
        let epoch = new_roster.epoch;
        let ctx = EpochContext::new(
            &group_key,
            epoch,
            new_roster
                .members
                .iter()
                .filter_map(|m| <[u8; 32]>::try_from(m.sign_pub.as_slice()).ok()),
        );
        let roster_update = envelope::seal_message(
            &dev.keys,
            &ctx,
            pb::payload_body::Kind::Control(pb::Control {
                kind: Some(pb::control::Kind::RosterUpdate(pb::RosterUpdate {
                    roster: Some(new_roster),
                })),
            }),
            now_ms,
        )?;

        Ok(Approval {
            welcome: inviter.welcome_payload(welcome),
            pair_channel: inviter.invite().pair_channel(),
            roster_update: roster_update.bytes,
            group_channel: ctx.channel_id().clone(),
            joiner,
        })
    }

    /// 초대로 그룹에 참여하는 쪽. 그룹이 없는 초기화된 기기여야 한다.
    pub fn join(&self, invite: Invite) -> Result<Joiner, Error> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        if dev.state.roster.is_some() {
            return Err(Error::GroupExists);
        }
        let keys = DeviceKeys::from_bytes(&dev.keys.to_bytes()[..])?;
        Ok(Joiner::new(invite, keys, &dev.state.device_name))
    }

    /// 참여가 끝나 받은 그룹을 저장한다.
    pub fn accept_join(&self, joined: JoinedGroup) -> Result<(), Error> {
        let mut slot = self.device.lock().unwrap();
        let dev = slot.as_mut().ok_or(Error::NotInitialized)?;
        if dev.state.roster.is_some() {
            return Err(Error::GroupExists);
        }
        let me = dev.keys.device_id().0;
        if joined.roster.epoch != joined.epoch
            || !joined.roster.members.iter().any(|m| m.device_id == me)
        {
            return Err(Error::BadGroup);
        }
        self.install_group(dev, joined.group_key, joined.roster)
    }

    /// 텍스트를 서명·봉인한다. 돌려받은 `bytes`를 `channel_id`에 게시한다.
    pub fn seal_text(&self, text: &str, now_ms: u64) -> Result<SealedMessage, Error> {
        if text.is_empty() {
            return Err(Error::EmptyText);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(Error::TextTooLarge);
        }
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        if dev.removed {
            return Err(Error::Removed);
        }
        let ctx = dev.send_context()?;
        let out = envelope::seal_message(
            &dev.keys,
            &ctx,
            pb::payload_body::Kind::Text(pb::ClipText {
                text: text.to_string(),
            }),
            now_ms,
        )?;
        Ok(SealedMessage {
            channel_id: ctx.channel_id().clone(),
            msg_id: out.msg_id,
            bytes: out.bytes,
        })
    }

    /// 이미지를 서명·봉인한다. 돌려받은 메시지를 **순서대로** `channel_id`에 게시한다
    /// (첫 메시지가 `ImageStart`, 나머지가 청크).
    pub fn seal_image(
        &self,
        bytes: &[u8],
        mime: &str,
        width: u32,
        height: u32,
        now_ms: u64,
    ) -> Result<Vec<SealedMessage>, Error> {
        let image = transfer::split(bytes, mime, width, height).map_err(|e| match e {
            ImageError::Empty => Error::EmptyImage,
            ImageError::TooLarge => Error::ImageTooLarge,
            ImageError::UnsupportedMime => Error::UnsupportedImage,
        })?;
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        if dev.removed {
            return Err(Error::Removed);
        }
        let ctx = dev.send_context()?;
        let kinds = std::iter::once(pb::payload_body::Kind::ImageStart(image.start)).chain(
            image
                .chunks
                .into_iter()
                .map(pb::payload_body::Kind::ImageChunk),
        );
        kinds
            .map(|kind| {
                let out = envelope::seal_message(&dev.keys, &ctx, kind, now_ms)?;
                Ok(SealedMessage {
                    channel_id: ctx.channel_id().clone(),
                    msg_id: out.msg_id,
                    bytes: out.bytes,
                })
            })
            .collect()
    }

    /// 릴레이에서 받은 페이로드를 검증하고 처리한다.
    /// 검증에 실패한 메시지는 `Err(이유)`로 돌려주며 이유별 카운터가 올라간다.
    pub fn handle_deliver(
        &self,
        payload: &[u8],
        source: Source,
        now_ms: u64,
    ) -> Result<Incoming, DropReason> {
        use pb::control::Kind as C;
        use pb::payload_body::Kind;

        let mut slot = self.device.lock().unwrap();
        let Some(dev) = slot.as_mut() else {
            return Ok(Incoming::Nothing);
        };
        if dev.removed {
            return Ok(Incoming::Nothing);
        }
        let Some(receiver) = dev.receiver.as_mut() else {
            return Ok(Incoming::Nothing);
        };
        let got = receiver.receive(payload, source, now_ms)?;

        let current_epoch = dev.state.roster.as_ref().map_or(0, |r| r.epoch);
        let sender_in_current = dev
            .state
            .roster
            .iter()
            .flat_map(|r| &r.members)
            .any(|m| m.device_id == got.sender.0);
        let is_query = matches!(
            &got.kind,
            Kind::Control(pb::Control {
                kind: Some(C::EpochQuery(_))
            })
        );

        // 이전 에포크로 온 메시지는 지금도 그룹에 있는 기기가 보낸 것만 받는다.
        // 제거된 기기가 옛 키로 계속 보내는 것을 막는다. 단, 돌아온 기기의 `EpochQuery`는 응답해야 한다.
        if got.epoch < current_epoch && !sender_in_current && !is_query {
            if let Some(receiver) = dev.receiver.as_mut() {
                receiver.count_drop(DropReason::SenderRemoved);
            }
            return Err(DropReason::SenderRemoved);
        }
        if got.epoch == current_epoch {
            self.touch(dev, now_ms);
        }

        let name_of = |dev: &Device| {
            dev.state
                .roster
                .iter()
                .flat_map(|r| &r.members)
                .find(|m| m.device_id == got.sender.0)
                .map(|m| m.name.clone())
                .unwrap_or_default()
        };
        match got.kind {
            Kind::ImageStart(s) => match dev.assembler.start(got.sender, &s, now_ms) {
                Ok(()) => Ok(Incoming::Nothing),
                Err(_) => Err(Self::drop_image(dev)),
            },
            Kind::ImageChunk(c) => match dev.assembler.chunk(got.sender, &c, now_ms) {
                Ok(None) => Ok(Incoming::Nothing),
                Ok(Some(image)) => Ok(Incoming::Image(ImageClip {
                    from: got.sender,
                    from_name: name_of(dev),
                    mime: image.mime,
                    bytes: image.bytes,
                    width: image.width,
                    height: image.height,
                    created_at_ms: got.created_at_ms,
                })),
                Err(_) => Err(Self::drop_image(dev)),
            },
            Kind::Text(t) => {
                let from_name = name_of(dev);
                Ok(Incoming::Clip(Clip {
                    from: got.sender,
                    from_name,
                    text: t.text,
                    created_at_ms: got.created_at_ms,
                }))
            }
            Kind::Control(pb::Control {
                kind: Some(C::RosterUpdate(update)),
            }) => {
                if got.epoch != current_epoch {
                    return Ok(Incoming::Nothing);
                }
                let changed = self.adopt_roster(dev, update.roster);
                Ok(if changed {
                    Incoming::DevicesChanged
                } else {
                    Incoming::Nothing
                })
            }
            Kind::Control(pb::Control {
                kind: Some(C::Rekey(rekey)),
            }) => Ok(self.handle_rekey(dev, got.sender, got.epoch, rekey, now_ms)),
            Kind::Control(pb::Control {
                kind: Some(C::EpochQuery(_)),
            }) => Ok(self.handle_query(dev, got.sender, got.epoch, now_ms)),
            _ => Ok(Incoming::Nothing),
        }
    }

    /// 이미지 조립에 실패한 메시지를 세고 이유를 돌려준다.
    fn drop_image(dev: &mut Device) -> DropReason {
        if let Some(receiver) = dev.receiver.as_mut() {
            receiver.count_drop(DropReason::BadImage);
        }
        DropReason::BadImage
    }

    /// 받은 `RosterUpdate`를 채택할지 정한다. 채택하면 저장하고 `true`.
    fn adopt_roster(&self, dev: &mut Device, new: Option<pb::Roster>) -> bool {
        let (Some(new), Some(current)) = (new, dev.state.roster.as_ref()) else {
            return false;
        };
        let me = dev.keys.device_id().0;
        if new.epoch != current.epoch
            || !roster_is_consistent(&new)
            // 내가 빠진 로스터는 기기 제거(키 회전)로 다룬다. 여기서는 채택하지 않는다.
            || !new.members.iter().any(|m| m.device_id == me)
            || !roster_wins(&new, current)
        {
            return false;
        }
        let mut state = dev.state.clone();
        state.roster = Some(new);
        if store::write_state(&self.state_path(), &dev.state_key, &state).is_err() {
            return false;
        }
        dev.state = state;
        dev.sync_receiver();
        true
    }

    /// 검증에 실패해 버린 메시지 수 (이유별).
    pub fn drop_counts(&self) -> std::collections::BTreeMap<DropReason, u64> {
        let slot = self.device.lock().unwrap();
        slot.as_ref()
            .and_then(|d| d.receiver.as_ref())
            .map(|r| r.drop_counts().clone())
            .unwrap_or_default()
    }

    pub fn status(&self) -> Result<Status, Error> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        Ok(Status {
            device_name: dev.state.device_name.clone(),
            device_id: dev.keys.device_id(),
            epoch: dev.state.roster.as_ref().map(|r| r.epoch),
            member_count: dev.state.roster.as_ref().map_or(0, |r| r.members.len()),
            health: dev.health(now_ms()),
        })
    }

    pub fn devices(&self) -> Vec<DeviceInfo> {
        let slot = self.device.lock().unwrap();
        let Some(dev) = slot.as_ref() else {
            return Vec::new();
        };
        let me = dev.keys.device_id();
        dev.state
            .roster
            .iter()
            .flat_map(|r| &r.members)
            .filter_map(|m| {
                let id = DeviceId(m.device_id.as_slice().try_into().ok()?);
                Some(DeviceInfo {
                    is_self: id == me,
                    device_id: id,
                    name: m.name.clone(),
                    added_at_ms: m.added_at_ms,
                })
            })
            .collect()
    }

    /// 그룹이 있으면 릴레이 접속에 필요한 채널 ID와 이 기기의 태그.
    pub fn connection(&self) -> Option<Connection> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref()?;
        if dev.removed {
            return None;
        }
        let epoch = dev.state.roster.as_ref()?.epoch;
        let group_key = dev.group_key.as_ref()?;
        Some(Connection {
            epoch,
            channel_id: group_key.channel_id(epoch),
            device_tag: group_key.device_tag(epoch, &dev.keys.device_id()),
        })
    }

    fn state_path(&self) -> PathBuf {
        self.config.data_dir.join("state.bin")
    }
}

fn load_group_key(secrets: &dyn SecretStore, epoch: u64) -> Result<GroupKey, Error> {
    let bytes = secrets
        .get(&group_key_name(epoch))
        .map(Zeroizing::new)
        .ok_or(Error::StateInconsistent)?;
    let key: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| Error::StateCorrupt)?;
    Ok(GroupKey::from_bytes(key))
}

mod epochs;
#[cfg(test)]
mod group_tests;
#[cfg(test)]
mod image_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod text_tests;
