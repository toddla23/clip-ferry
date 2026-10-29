//! 셸이 쓰는 공개 API.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use zeroize::Zeroizing;

use crate::crypto::{ChannelId, CryptoError, DeviceId, DeviceKeys, DeviceTag, GroupKey, StateKey};
use crate::envelope::{self, EnvelopeError, EpochContext};
use crate::group;
use crate::pairing::{
    self, Identity, Invite, Inviter, JoinedGroup, Joiner, JoinerInfo, PairingError,
};
use crate::pb;
use crate::store::{self, SecretStore, StateFileError, StoreError};

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

/// 릴레이에 접속할 때 쓰는 값.
#[derive(Debug, Clone)]
pub struct Connection {
    pub epoch: u64,
    pub channel_id: ChannelId,
    pub device_tag: DeviceTag,
}

struct Device {
    keys: DeviceKeys,
    state_key: StateKey,
    state: pb::LocalState,
    group_key: Option<GroupKey>,
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
                Some(Device {
                    keys,
                    state_key,
                    state,
                    group_key,
                })
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

        self.secrets
            .set(&group_key_name(epoch), group_key.as_bytes())?;
        if let Err(e) = store::write_state(&self.state_path(), &dev.state_key, &state) {
            let _ = self.secrets.delete(&group_key_name(epoch));
            return Err(e.into());
        }
        dev.state = state;
        dev.group_key = Some(group_key);
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
        let group_key = dev.group_key.as_ref().ok_or(Error::NoGroup)?;

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
        let welcome = pairing::build_welcome(&dev.keys, &joiner, group_key, &new_roster)?;

        let mut state = dev.state.clone();
        state.roster = Some(new_roster.clone());
        store::write_state(&self.state_path(), &dev.state_key, &state)?;
        dev.state = state;

        // 그룹의 다른 기기들에게 알릴 `RosterUpdate`
        let epoch = new_roster.epoch;
        let ctx = EpochContext::new(
            group_key,
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

    pub fn status(&self) -> Result<Status, Error> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        Ok(Status {
            device_name: dev.state.device_name.clone(),
            device_id: dev.keys.device_id(),
            epoch: dev.state.roster.as_ref().map(|r| r.epoch),
            member_count: dev.state.roster.as_ref().map_or(0, |r| r.members.len()),
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

#[cfg(test)]
mod tests;
