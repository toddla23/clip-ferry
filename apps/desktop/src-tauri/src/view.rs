//! 화면(UI)에 보내는 상태 요약. 비밀값은 없다.

use clip_ferry_core::api::{ClipFerry, Error, GroupHealth};
use clip_ferry_core::session::ConnectionStatus;
use qrcode::{Color, QrCode};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Connection {
    Connected,
    Reconnecting,
    Disconnected,
    /// 세션을 시작하지 못했다 (제거됨, 오류, 아직 그룹 없음 등).
    #[default]
    Stopped,
}

impl From<ConnectionStatus> for Connection {
    fn from(s: ConnectionStatus) -> Self {
        match s {
            ConnectionStatus::Connected => Connection::Connected,
            ConnectionStatus::Disconnected => Connection::Disconnected,
            ConnectionStatus::Reconnecting => Connection::Reconnecting,
        }
    }
}

impl Connection {
    pub fn label(self) -> &'static str {
        match self {
            Connection::Connected => "연결됨",
            Connection::Reconnecting => "재연결 중",
            Connection::Disconnected => "연결 끊김",
            Connection::Stopped => "중지됨",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    #[default]
    Active,
    Removed,
    Stale,
}

impl From<GroupHealth> for Health {
    fn from(h: GroupHealth) -> Self {
        match h {
            GroupHealth::Active => Health::Active,
            GroupHealth::Removed => Health::Removed,
            GroupHealth::Stale => Health::Stale,
        }
    }
}

/// 그룹이 아직 없으면 `Setup`(첫 실행 마법사), 있으면 `Ready`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    #[default]
    Setup,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    /// 앞 4바이트(8자리 hex)만.
    pub id: String,
    pub name: String,
    pub is_self: bool,
}

/// QR 코드. `path`는 어두운 칸마다 1×1 사각형을 그리는 SVG 경로이고, 그림 크기는 `width`×`width`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Qr {
    pub width: usize,
    pub path: String,
}

pub fn qr(data: &str) -> Option<Qr> {
    let code = QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let mut path = String::new();
    for (i, c) in code.to_colors().into_iter().enumerate() {
        if c == Color::Dark {
            path.push_str(&format!("M{} {}h1v1h-1z", i % width, i / width));
        }
    }
    Some(Qr { width, path })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PairKind {
    Invite,
    Join,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PairPhase {
    /// 참여 쪽: 릴레이에 연결하는 중
    Connecting,
    /// 초대 쪽: 상대를 기다리는 중 / 참여 쪽: 초대한 기기의 승인을 기다리는 중
    Waiting,
    /// 초대 쪽: 참여 요청이 와서 확인 숫자를 보여 주고 승인을 받는 중
    Request,
    Done,
    Failed,
}

/// 진행 중인 페어링 화면에 필요한 값.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingView {
    pub kind: PairKind,
    pub phase: PairPhase,
    pub text_code: Option<String>,
    pub qr: Option<Qr>,
    pub expires_at_ms: Option<u64>,
    /// 상대 기기 이름
    pub peer_name: Option<String>,
    /// 6자리 확인 숫자
    pub sas: Option<String>,
    pub message: Option<String>,
}

impl PairingView {
    pub fn new(kind: PairKind, phase: PairPhase) -> Self {
        Self {
            kind,
            phase,
            text_code: None,
            qr: None,
            expires_at_ms: None,
            peer_name: None,
            sas: None,
            message: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    /// 만들 때마다 커지는 번호. 화면은 더 작은 번호의 상태를 버린다(응답과 이벤트의 도착 순서가 바뀔 수 있다).
    pub seq: u64,
    pub stage: Stage,
    pub connection: Connection,
    pub device_name: String,
    /// 앞 4바이트(8자리 hex)만.
    pub device_id: String,
    pub epoch: Option<u64>,
    pub member_count: usize,
    pub devices: Vec<DeviceView>,
    pub health: Health,
    pub relay_url: String,
    pub error: Option<String>,
    pub pairing: Option<PairingView>,
    /// 마법사에서 이름 칸에 미리 넣어 줄 값.
    pub default_name: String,
}

pub fn short_id(id: &[u8; 16]) -> String {
    id[..4].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn build(
    app: &ClipFerry,
    connection: Connection,
    relay_url: &str,
    error: Option<String>,
) -> AppView {
    let base = AppView {
        relay_url: relay_url.to_string(),
        ..AppView::default()
    };
    match app.status() {
        Ok(s) => {
            let ready = s.epoch.is_some();
            AppView {
                stage: if ready { Stage::Ready } else { Stage::Setup },
                // 그룹에서 제거되었으면 릴레이에 붙어 있어도 동작하지 않는 상태다.
                connection: if ready && s.health == GroupHealth::Active {
                    connection
                } else {
                    Connection::Stopped
                },
                device_name: s.device_name,
                device_id: short_id(&s.device_id.0),
                epoch: s.epoch,
                member_count: s.member_count,
                devices: app
                    .devices()
                    .into_iter()
                    .map(|d| DeviceView {
                        id: short_id(&d.device_id.0),
                        name: d.name,
                        is_self: d.is_self,
                    })
                    .collect(),
                health: s.health.into(),
                error,
                ..base
            }
        }
        // 아직 기기를 만들기 전이다. 오류가 아니라 첫 실행이다.
        Err(Error::NotInitialized) => AppView { error, ..base },
        Err(e) => AppView {
            error: Some(error.unwrap_or_else(|| e.to_string())),
            ..base
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_ferry_core::api::Config;
    use clip_ferry_core::store::MemorySecretStore;

    fn open(dir: &std::path::Path) -> ClipFerry {
        ClipFerry::open(
            Config {
                data_dir: dir.to_path_buf(),
            },
            Box::new(MemorySecretStore::default()),
        )
        .unwrap()
    }

    #[test]
    fn short_id_is_eight_hex_chars() {
        let mut id = [0u8; 16];
        id[..4].copy_from_slice(&[0xab, 0x01, 0xff, 0x10]);
        assert_eq!(short_id(&id), "ab01ff10");
    }

    #[test]
    fn first_run_is_setup_without_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let v = build(&open(dir.path()), Connection::Connected, "ws://x", None);
        assert_eq!(v.stage, Stage::Setup);
        assert_eq!(v.connection, Connection::Stopped);
        assert_eq!(v.error, None);
        assert!(v.devices.is_empty());
    }

    #[test]
    fn device_without_a_group_is_still_setup() {
        let dir = tempfile::tempdir().unwrap();
        let app = open(dir.path());
        app.init_device("desk").unwrap();
        let v = build(&app, Connection::Connected, "ws://x", None);
        assert_eq!(v.stage, Stage::Setup);
        assert_eq!(v.device_name, "desk");
        assert_eq!(v.connection, Connection::Stopped);
    }

    #[test]
    fn view_after_group_creation_has_no_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let app = open(dir.path());
        app.init_device("desk").unwrap();
        app.create_group().unwrap();
        let v = build(&app, Connection::Reconnecting, "ws://x", None);
        assert_eq!(v.stage, Stage::Ready);
        assert_eq!(v.device_name, "desk");
        assert_eq!(v.device_id.len(), 8);
        assert_eq!((v.epoch, v.member_count), (Some(1), 1));
        assert_eq!(v.connection, Connection::Reconnecting);
        assert_eq!(v.health, Health::Active);
        assert_eq!(v.devices.len(), 1);
        assert!(v.devices[0].is_self && v.devices[0].id == v.device_id);
    }

    #[test]
    fn serializes_in_the_shape_the_ui_expects() {
        let dir = tempfile::tempdir().unwrap();
        let app = open(dir.path());
        app.init_device("desk").unwrap();
        app.create_group().unwrap();
        let mut view = build(&app, Connection::Connected, "ws://x", None);
        view.pairing = Some(PairingView::new(PairKind::Invite, PairPhase::Request));
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["connection"], "connected");
        assert_eq!(json["health"], "active");
        assert_eq!(json["stage"], "ready");
        assert_eq!(json["pairing"]["kind"], "invite");
        assert_eq!(json["pairing"]["phase"], "request");
        assert!(json["devices"][0].get("isSelf").is_some());
        assert!(json.get("deviceName").is_some() && json.get("relayUrl").is_some());
        assert!(json.get("defaultName").is_some());
    }

    #[test]
    fn qr_covers_the_code_with_a_path() {
        let q = qr("clipferry://pair/1/AAAA").unwrap();
        assert!(q.width >= 21);
        assert!(q.path.starts_with('M') && q.path.contains("h1v1h-1z"));
    }
}
