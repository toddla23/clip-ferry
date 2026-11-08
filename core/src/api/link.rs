//! 계정 연결(4.4, F37)의 기기 쪽 처리. 메시지 형식은 `crate::link`.

use super::*;
use crate::link::{self, LinkRequestInfo};

impl ClipFerry {
    /// 이 기기의 공개키와 이름: 계정 서버에 참여 확인서를 받을 때 보낸다.
    /// 돌려주는 값은 `(sign_pub, dh_pub, name)`.
    pub fn link_identity(&self) -> Result<([u8; 32], [u8; 32], String), Error> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        Ok((
            dev.keys.sign_public(),
            dev.keys.dh_public(),
            dev.state.device_name.clone(),
        ))
    }

    /// 이미 그룹에 있는 기기인지.
    pub fn is_member(&self, device_id: &DeviceId) -> bool {
        let slot = self.device.lock().unwrap();
        slot.as_ref()
            .and_then(|d| d.state.roster.as_ref())
            .is_some_and(|r| r.members.iter().any(|m| m.device_id == device_id.0))
    }

    /// 확인을 마친 참여 요청에 응한다. 봉인 후 공개 방식의 초대와, 계정 채널에 게시할
    /// `LinkInvite`를 돌려준다.
    pub fn create_link_invite(&self, req: &LinkRequestInfo) -> Result<(Inviter, Vec<u8>), Error> {
        if self.is_member(&req.device_id()) {
            return Err(PairingError::AlreadyMember.into());
        }
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        if dev.state.roster.is_none() {
            return Err(Error::NoGroup);
        }
        let invite = Invite::generate(None);
        let message = link::build_invite(req, &invite);
        let inviter = Inviter::new_committed(
            invite,
            Identity::of(&dev.keys, &dev.state.device_name),
            now_ms(),
        );
        Ok((inviter, message))
    }

    /// 계정 채널에서 받은 바이트가 내 요청(`request_id`)에 대한 `LinkInvite`면 참여를 시작한다.
    /// 해당하지 않으면 `Ok(None)`.
    pub fn link_join(
        &self,
        payload: &[u8],
        request_id: &[u8; 16],
    ) -> Result<Option<Joiner>, Error> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        if dev.state.roster.is_some() {
            return Err(Error::GroupExists);
        }
        let Some(secret) = link::open_invite(payload, request_id, &dev.keys) else {
            return Ok(None);
        };
        let keys = DeviceKeys::from_bytes(&dev.keys.to_bytes()[..])?;
        Ok(Some(Joiner::new_committed(
            Invite::from_secret(secret, None),
            keys,
            &dev.state.device_name,
        )))
    }
}

#[cfg(test)]
impl ClipFerry {
    pub(crate) fn device_keys_for_test(&self) -> DeviceKeys {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().expect("초기화된 기기");
        DeviceKeys::from_bytes(&dev.keys.to_bytes()[..]).unwrap()
    }
}
