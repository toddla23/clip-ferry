//! 화면(UI)에 보내는 상태 요약. 비밀값은 없다.

use clip_ferry_core::api::{ClipFerry, GroupHealth};
use clip_ferry_core::session::ConnectionStatus;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Connection {
    Connected,
    Reconnecting,
    Disconnected,
    /// 세션을 시작하지 못했다 (제거됨, 오류 등).
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    /// 만들 때마다 커지는 번호. 화면은 더 작은 번호의 상태를 버린다(응답과 이벤트의 도착 순서가 바뀔 수 있다).
    pub seq: u64,
    pub connection: Connection,
    pub device_name: String,
    /// 앞 4바이트(8자리 hex)만.
    pub device_id: String,
    pub epoch: Option<u64>,
    pub member_count: usize,
    pub health: Health,
    pub relay_url: String,
    pub error: Option<String>,
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
    match app.status() {
        Ok(s) => AppView {
            seq: 0,
            // 그룹에서 제거되었으면 릴레이에 붙어 있어도 동작하지 않는 상태다.
            connection: if s.health == GroupHealth::Active {
                connection
            } else {
                Connection::Stopped
            },
            device_name: s.device_name,
            device_id: short_id(&s.device_id.0),
            epoch: s.epoch,
            member_count: s.member_count,
            health: s.health.into(),
            relay_url: relay_url.to_string(),
            error,
        },
        Err(e) => AppView {
            seq: 0,
            connection: Connection::Stopped,
            device_name: String::new(),
            device_id: String::new(),
            epoch: None,
            member_count: 0,
            health: Health::Active,
            relay_url: relay_url.to_string(),
            error: Some(error.unwrap_or_else(|| e.to_string())),
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
    fn view_before_init_is_stopped_with_error() {
        let dir = tempfile::tempdir().unwrap();
        let v = build(&open(dir.path()), Connection::Connected, "ws://x", None);
        assert_eq!(v.connection, Connection::Stopped);
        assert!(v.error.is_some());
    }

    #[test]
    fn view_after_group_creation_has_no_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let app = open(dir.path());
        app.init_device("desk").unwrap();
        app.create_group().unwrap();
        let v = build(&app, Connection::Reconnecting, "ws://x", None);
        assert_eq!(v.device_name, "desk");
        assert_eq!(v.device_id.len(), 8);
        assert_eq!((v.epoch, v.member_count), (Some(1), 1));
        assert_eq!(v.connection, Connection::Reconnecting);
        assert_eq!(v.health, Health::Active);
    }

    #[test]
    fn serializes_in_the_shape_the_ui_expects() {
        let dir = tempfile::tempdir().unwrap();
        let app = open(dir.path());
        app.init_device("desk").unwrap();
        app.create_group().unwrap();
        let json =
            serde_json::to_value(build(&app, Connection::Connected, "ws://x", None)).unwrap();
        assert_eq!(json["connection"], "connected");
        assert_eq!(json["health"], "active");
        assert!(json.get("deviceName").is_some() && json.get("relayUrl").is_some());
    }
}
