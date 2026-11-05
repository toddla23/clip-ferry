//! 푸시 토큰과 재전송 요청 (F22·F25, 03-protocol 5·6절, D-016).
//!
//! - 푸시를 받을 기기(iOS)는 자기 토큰을 로스터의 `Member.push`에 적어 `RosterUpdate`로 알린다.
//! - 보내는 기기는 게시할 때 토큰이 있는 다른 구성원을 `PushTarget`으로 붙인다. 릴레이는 그 기기가
//!   오프라인일 때만 APNs로 보내고 토큰은 바로 잊는다.
//! - 푸시에 다 못 담은 것(이미지, 긴 텍스트)은 받는 기기가 `ResendRequest`를 보낸 기기에게만 게시한다.

use super::*;
use crate::envelope::seal_message;
use pb::control::Kind as C;
use pb::payload_body::Kind;

/// 푸시 토큰 최대 길이(바이트). APNs 토큰은 64자 16진수다.
pub const MAX_PUSH_TOKEN: usize = 256;

impl ClipFerry {
    /// 이 기기의 푸시 토큰을 로스터에 적는다. 빈 토큰은 지운다.
    /// 바뀌었으면 그룹 채널에 게시할 `RosterUpdate`를, 그대로면 `None`을 돌려준다.
    pub fn set_push_token(
        &self,
        platform: pb::Platform,
        token: &str,
        now_ms: u64,
    ) -> Result<Option<SealedMessage>, Error> {
        if token.len() > MAX_PUSH_TOKEN {
            return Err(Error::BadPushToken);
        }
        let push = (!token.is_empty()).then(|| pb::PushInfo {
            platform: platform as i32,
            token: token.to_string(),
        });
        let mut slot = self.device.lock().unwrap();
        let dev = slot.as_mut().ok_or(Error::NotInitialized)?;
        if dev.removed {
            return Err(Error::Removed);
        }
        let mut roster = dev.state.roster.clone().ok_or(Error::NoGroup)?;
        let me = dev.keys.device_id();
        let member = roster
            .members
            .iter_mut()
            .find(|m| m.device_id == me.0)
            .ok_or(Error::BadGroup)?;
        if member.push == push {
            return Ok(None);
        }
        member.push = push;
        roster.version += 1;

        let mut state = dev.state.clone();
        state.roster = Some(roster.clone());
        store::write_state(&self.state_path(), &dev.state_key, &state)?;
        dev.state = state;
        dev.sync_receiver();

        let ctx = dev.send_context()?;
        let out = seal_message(
            &dev.keys,
            &ctx,
            Kind::Control(pb::Control {
                kind: Some(C::RosterUpdate(pb::RosterUpdate {
                    roster: Some(roster),
                })),
            }),
            now_ms,
        )?;
        Ok(Some(SealedMessage {
            channel_id: ctx.channel_id().clone(),
            msg_id: out.msg_id,
            bytes: out.bytes,
        }))
    }

    /// 현재 에포크에서 푸시 토큰을 적어 둔 다른 구성원들. 텍스트·`ImageStart`를 게시할 때 붙인다.
    pub fn push_targets(&self) -> Vec<pb::PushTarget> {
        let slot = self.device.lock().unwrap();
        let Some(dev) = slot.as_ref() else {
            return Vec::new();
        };
        let (Some(group_key), Some(roster)) = (&dev.group_key, &dev.state.roster) else {
            return Vec::new();
        };
        let me = dev.keys.device_id();
        roster
            .members
            .iter()
            .filter(|m| m.device_id != me.0)
            .filter_map(|m| {
                let push = m.push.as_ref().filter(|p| !p.token.is_empty())?;
                let id = DeviceId(m.device_id.as_slice().try_into().ok()?);
                Some(pb::PushTarget {
                    device_tag: group_key.device_tag(roster.epoch, &id).0.to_vec(),
                    platform: push.platform,
                    token: push.token.clone(),
                })
            })
            .collect()
    }

    /// 현재 에포크에서 그 기기의 태그 (`Publish.to`에 쓴다).
    pub fn device_tag_of(&self, device: &DeviceId) -> Option<DeviceTag> {
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref()?;
        let epoch = dev.state.roster.as_ref()?.epoch;
        Some(dev.group_key.as_ref()?.device_tag(epoch, device))
    }

    /// 현재 에포크의 태그로 구성원을 찾는다 (페이로드 없는 푸시의 `from` 태그용).
    pub fn device_for_tag(&self, tag: &[u8]) -> Option<DeviceInfo> {
        self.devices()
            .into_iter()
            .find(|d| self.device_tag_of(&d.device_id).is_some_and(|t| t.0 == tag))
    }

    /// `ResendRequest`를 봉인한다. 돌려받은 `bytes`를 보낸 기기에게만(`to`) 게시한다.
    /// `msg_id`가 비어 있으면 "마지막으로 보낸 것"을 뜻한다.
    pub fn seal_resend_request(&self, msg_id: &[u8], now_ms: u64) -> Result<SealedMessage, Error> {
        if !(msg_id.is_empty() || msg_id.len() == 16) {
            return Err(Error::BadMsgId);
        }
        let slot = self.device.lock().unwrap();
        let dev = slot.as_ref().ok_or(Error::NotInitialized)?;
        if dev.removed {
            return Err(Error::Removed);
        }
        let ctx = dev.send_context()?;
        let out = seal_message(
            &dev.keys,
            &ctx,
            Kind::Control(pb::Control {
                kind: Some(C::Resend(pb::ResendRequest {
                    msg_id: msg_id.to_vec(),
                })),
            }),
            now_ms,
        )?;
        Ok(SealedMessage {
            channel_id: ctx.channel_id().clone(),
            msg_id: out.msg_id,
            bytes: out.bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::text_tests::{clip, pair, profile, Profile, T0};
    use super::*;

    fn deliver(app: &ClipFerry, bytes: &[u8], now: u64) -> Result<Incoming, DropReason> {
        app.handle_deliver(bytes, Source::Relay, now)
    }

    fn two() -> (Profile, Profile) {
        let a = profile("laptop", true);
        let b = profile("iphone", false);
        pair(&a, &b);
        (a, b)
    }

    #[test]
    fn push_token_goes_into_roster_and_targets() {
        let (a, b) = two();
        assert!(a.app.push_targets().is_empty());

        let update = b
            .app
            .set_push_token(pb::Platform::Apns, "abcd", T0)
            .unwrap()
            .expect("바뀌었으니 RosterUpdate");
        assert_eq!(
            deliver(&a.app, &update.bytes, T0),
            Ok(Incoming::DevicesChanged)
        );

        let targets = a.app.push_targets();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].token, "abcd");
        assert_eq!(targets[0].platform, pb::Platform::Apns as i32);
        let b_id = b.app.status().unwrap().device_id;
        assert_eq!(
            targets[0].device_tag,
            a.app.device_tag_of(&b_id).unwrap().0.to_vec()
        );
        // 자기 토큰은 자기 대상에 없다.
        assert!(b.app.push_targets().is_empty());
        // 같은 토큰이면 다시 알리지 않는다.
        assert!(b
            .app
            .set_push_token(pb::Platform::Apns, "abcd", T0)
            .unwrap()
            .is_none());
        // 태그로 기기를 찾는다.
        assert_eq!(
            a.app
                .device_for_tag(&targets[0].device_tag)
                .unwrap()
                .device_id,
            b_id
        );
        assert!(a.app.device_for_tag(&[0; 16]).is_none());

        // 빈 토큰은 지운다.
        let clear = b
            .app
            .set_push_token(pb::Platform::Apns, "", T0 + 1)
            .unwrap()
            .unwrap();
        assert_eq!(
            deliver(&a.app, &clear.bytes, T0 + 1),
            Ok(Incoming::DevicesChanged)
        );
        assert!(a.app.push_targets().is_empty());
    }

    #[test]
    fn push_token_too_long_is_refused() {
        let (_a, b) = two();
        let long = "x".repeat(MAX_PUSH_TOKEN + 1);
        assert!(matches!(
            b.app.set_push_token(pb::Platform::Apns, &long, T0),
            Err(Error::BadPushToken)
        ));
    }

    #[test]
    fn resend_request_reaches_sender() {
        let (a, b) = two();
        let sent = a.app.seal_text("안녕", T0).unwrap();
        let got = clip(deliver(&b.app, &sent.bytes, T0));
        assert_eq!(got.msg_id, sent.msg_id);

        let req = b.app.seal_resend_request(&got.msg_id, T0 + 1).unwrap();
        let b_id = b.app.status().unwrap().device_id;
        assert_eq!(
            deliver(&a.app, &req.bytes, T0 + 1),
            Ok(Incoming::ResendRequested {
                from: b_id,
                msg_id: sent.msg_id.to_vec()
            })
        );
        assert!(b.app.seal_resend_request(&[1, 2, 3], T0).is_err());
        assert!(b.app.seal_resend_request(&[], T0).is_ok());
    }

    #[test]
    fn image_start_by_push_is_announced() {
        let (a, b) = two();
        let bytes = vec![7u8; 1000];
        let msgs = a.app.seal_image(&bytes, "image/png", 1, 1, T0).unwrap();
        let a_id = a.app.status().unwrap().device_id;
        let got = b.app.handle_deliver(&msgs[0].bytes, Source::Push, T0);
        let Ok(Incoming::ImageAnnounced {
            from,
            from_name,
            msg_id,
        }) = got
        else {
            panic!("{got:?}");
        };
        assert_eq!(from, a_id);
        assert_eq!(from_name, "laptop");
        assert_eq!(msg_id, msgs[0].msg_id);
    }
}
