//! 모바일 셸(Kotlin/Swift)용 UniFFI 바인딩.
//!
//! 코어(`clip-ferry-core`)의 비동기 세션을 셸이 쓰기 쉬운 동기 호출과 콜백으로 감싼다.
//! 암호·프로토콜 로직은 없다. tokio 런타임을 하나 쥐고, 코어를 부르고, 이벤트를 셸에 알릴 뿐이다.
//! F15 범위: 기기·그룹 만들기, 상태 조회, 릴레이 세션 시작·중지, 연결 상태 알림.
//! 페어링(F16)과 클립보드 송수신(F17~)은 이후 기능에서 같은 방식으로 추가한다.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clip_ferry_core::api::{ClipFerry, Config, Error, GroupHealth};
use clip_ferry_core::crypto::DeviceId;
use clip_ferry_core::session::{ConnectionStatus, EventSink, Session};
use clip_ferry_core::store::{SecretStore, StoreError};
use clip_ferry_core::transport::TransportConfig;

uniffi::setup_scaffolding!();

/// 셸에 넘기는 오류. 메시지에는 키·내용이 들어 있지 않다(코어 오류 규칙과 같다).
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// 이 기기가 그룹에서 제거되었다. 다시 페어링해야 한다.
    #[error("이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요")]
    Removed,
    #[error("{reason}")]
    Failed { reason: String },
}

impl From<Error> for FfiError {
    fn from(e: Error) -> Self {
        match e {
            Error::Removed => FfiError::Removed,
            other => FfiError::Failed {
                reason: other.to_string(),
            },
        }
    }
}

/// 셸이 구현한 콜백이 예외를 던졌을 때(예: Keystore 오류) 오류로 바꾼다.
impl From<uniffi::UnexpectedUniFFICallbackError> for FfiError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        FfiError::Failed { reason: e.reason }
    }
}

/// 플랫폼 보안 저장소. 셸이 구현한다 (Android: Keystore로 감싼 저장소).
#[uniffi::export(with_foreign)]
pub trait SecretVault: Send + Sync {
    fn get(&self, key: String) -> Option<Vec<u8>>;
    fn set(&self, key: String, value: Vec<u8>) -> Result<(), FfiError>;
    fn delete(&self, key: String) -> Result<(), FfiError>;
}

/// 셸의 `SecretVault`를 코어의 `SecretStore`로 이어 준다.
struct VaultAdapter(Arc<dyn SecretVault>);

fn backend(e: FfiError) -> StoreError {
    StoreError::Backend(e.to_string())
}

impl SecretStore for VaultAdapter {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.0.get(key.to_string())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.0.set(key.to_string(), value.to_vec()).map_err(backend)
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.0.delete(key.to_string()).map_err(backend)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LinkStatus {
    Connected,
    Disconnected,
    Reconnecting,
}

impl From<ConnectionStatus> for LinkStatus {
    fn from(s: ConnectionStatus) -> Self {
        match s {
            ConnectionStatus::Connected => LinkStatus::Connected,
            ConnectionStatus::Disconnected => LinkStatus::Disconnected,
            ConnectionStatus::Reconnecting => LinkStatus::Reconnecting,
        }
    }
}

/// 그룹과의 관계.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Health {
    Active,
    /// 제거되었다. 다시 페어링해야 한다.
    Removed,
    /// 30일 넘게 연락이 없다. 다시 페어링해야 할 수 있다.
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

/// 화면에 보여도 되는 요약. 비밀값은 없다.
#[derive(Debug, Clone, uniffi::Record)]
pub struct StatusInfo {
    pub device_name: String,
    /// 기기 id 앞 4바이트(hex). 화면 표시용.
    pub device_id_short: String,
    /// 그룹이 있을 때만.
    pub epoch: Option<u64>,
    pub member_count: u32,
    pub health: Health,
}

/// 셸이 구현하는 알림 창구. 코어의 어느 스레드에서든 불릴 수 있다.
#[uniffi::export(with_foreign)]
pub trait CoreEvents: Send + Sync {
    fn on_status(&self, status: LinkStatus);
    /// 그룹 채널에 붙어 있는 기기 수 (이 기기 포함). 연결이 끊기면 0.
    fn on_online_count(&self, count: u32);
    /// 기기 목록·에포크·그룹 상태 중 무언가 바뀌었다. 화면은 `status()`로 다시 읽는다.
    fn on_changed(&self);
}

/// 코어 이벤트를 셸 콜백으로 옮긴다. 받은 클립은 F17 전까지 다루지 않는다.
struct EventAdapter(Arc<dyn CoreEvents>);

impl EventSink for EventAdapter {
    fn on_clip(&self, _clip: clip_ferry_core::api::Clip) {}

    fn on_status(&self, status: ConnectionStatus) {
        self.0.on_status(status.into());
    }

    fn on_devices_changed(&self) {
        self.0.on_changed();
    }

    fn on_epoch_changed(&self, _epoch: u64) {
        self.0.on_changed();
    }

    fn on_group_health(&self, _health: GroupHealth) {
        self.0.on_changed();
    }

    fn on_presence(&self, online: Vec<DeviceId>) {
        self.0.on_online_count(online.len() as u32);
    }
}

fn short_id(id: &DeviceId) -> String {
    id.0[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// 셸이 쥐는 코어 핸들. tokio 런타임과 (연결 중이면) 릴레이 세션을 함께 들고 있다.
#[derive(uniffi::Object)]
pub struct ClipFerryClient {
    app: Arc<ClipFerry>,
    runtime: tokio::runtime::Runtime,
    session: Mutex<Option<Session>>,
}

#[uniffi::export]
impl ClipFerryClient {
    /// `data_dir`에 `state.bin`을 두고 열어 본다. 저장된 기기가 있으면 불러온다.
    #[uniffi::constructor]
    pub fn open(data_dir: String, vault: Arc<dyn SecretVault>) -> Result<Arc<Self>, FfiError> {
        let dir = PathBuf::from(data_dir);
        std::fs::create_dir_all(&dir).map_err(|_| FfiError::Failed {
            reason: "데이터 폴더를 만들 수 없습니다".into(),
        })?;
        let app = ClipFerry::open(Config { data_dir: dir }, Box::new(VaultAdapter(vault)))?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("clip-ferry")
            .build()
            .map_err(|_| FfiError::Failed {
                reason: "비동기 런타임을 만들 수 없습니다".into(),
            })?;
        Ok(Arc::new(Self {
            app: Arc::new(app),
            runtime,
            session: Mutex::new(None),
        }))
    }

    pub fn is_initialized(&self) -> bool {
        self.app.is_initialized()
    }

    /// 기기 키와 로컬 저장 키를 만들고 이름을 정한다.
    pub fn init_device(&self, device_name: String) -> Result<(), FfiError> {
        Ok(self.app.init_device(&device_name)?)
    }

    /// 에포크 1의 새 그룹을 만든다. 구성원은 이 기기뿐이다.
    pub fn create_group(&self) -> Result<(), FfiError> {
        Ok(self.app.create_group()?)
    }

    /// 릴레이에 연결할 수 있는 그룹이 있는가 (제거된 기기는 `false`).
    pub fn has_group(&self) -> bool {
        self.app.connection().is_some()
    }

    pub fn status(&self) -> Result<StatusInfo, FfiError> {
        let s = self.app.status()?;
        Ok(StatusInfo {
            device_name: s.device_name,
            device_id_short: short_id(&s.device_id),
            epoch: s.epoch,
            member_count: s.member_count as u32,
            health: s.health.into(),
        })
    }

    /// 릴레이에 (다시) 연결하는 세션을 시작한다. 이미 있으면 닫고 새로 연다.
    /// 연결 결과는 `events`로 온다.
    pub fn start(&self, relay_url: String, events: Arc<dyn CoreEvents>) -> Result<(), FfiError> {
        let url = relay_url.trim();
        if !(url.starts_with("ws://") || url.starts_with("wss://")) {
            return Err(FfiError::Failed {
                reason: "릴레이 주소는 ws:// 또는 wss://로 시작해야 합니다".into(),
            });
        }
        let _guard = self.runtime.enter();
        let mut slot = self.session.lock().unwrap();
        *slot = None;
        let session = self.app.start_session(
            url,
            TransportConfig::default(),
            Arc::new(EventAdapter(events)),
        )?;
        *slot = Some(session);
        Ok(())
    }

    /// 세션을 닫는다. 연결 중이 아니면 아무 일도 하지 않는다.
    pub fn stop(&self) {
        let _guard = self.runtime.enter();
        *self.session.lock().unwrap() = None;
    }

    pub fn is_running(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }
}

/// 코어 버전 문자열.
#[uniffi::export]
pub fn core_version() -> String {
    clip_ferry_core::version().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemVault(Mutex<HashMap<String, Vec<u8>>>);

    impl SecretVault for MemVault {
        fn get(&self, key: String) -> Option<Vec<u8>> {
            self.0.lock().unwrap().get(&key).cloned()
        }
        fn set(&self, key: String, value: Vec<u8>) -> Result<(), FfiError> {
            self.0.lock().unwrap().insert(key, value);
            Ok(())
        }
        fn delete(&self, key: String) -> Result<(), FfiError> {
            self.0.lock().unwrap().remove(&key);
            Ok(())
        }
    }

    #[derive(Default)]
    struct Recorder {
        statuses: Mutex<Vec<LinkStatus>>,
    }

    impl CoreEvents for Recorder {
        fn on_status(&self, status: LinkStatus) {
            self.statuses.lock().unwrap().push(status);
        }
        fn on_online_count(&self, _count: u32) {}
        fn on_changed(&self) {}
    }

    #[test]
    fn creates_device_and_group_then_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemVault::default());
        let client =
            ClipFerryClient::open(dir.path().display().to_string(), vault.clone()).unwrap();
        assert!(!client.is_initialized());
        client.init_device("폰".into()).unwrap();
        client.create_group().unwrap();
        assert!(client.has_group());
        let s = client.status().unwrap();
        assert_eq!(s.device_name, "폰");
        assert_eq!(s.epoch, Some(1));
        assert_eq!(s.member_count, 1);
        assert_eq!(s.health, Health::Active);
        assert_eq!(s.device_id_short.len(), 8);

        // 같은 폴더·저장소로 다시 열면 같은 기기다.
        let again = ClipFerryClient::open(dir.path().display().to_string(), vault).unwrap();
        assert_eq!(again.status().unwrap().device_id_short, s.device_id_short);
    }

    #[test]
    fn rejects_non_websocket_relay_url() {
        let dir = tempfile::tempdir().unwrap();
        let client = ClipFerryClient::open(
            dir.path().display().to_string(),
            Arc::new(MemVault::default()),
        )
        .unwrap();
        client.init_device("폰".into()).unwrap();
        client.create_group().unwrap();
        let err = client
            .start(
                "http://127.0.0.1:8787".into(),
                Arc::new(Recorder::default()),
            )
            .unwrap_err();
        assert!(matches!(err, FfiError::Failed { .. }));
        assert!(!client.is_running());
    }

    #[test]
    fn start_without_group_fails() {
        let dir = tempfile::tempdir().unwrap();
        let client = ClipFerryClient::open(
            dir.path().display().to_string(),
            Arc::new(MemVault::default()),
        )
        .unwrap();
        client.init_device("폰".into()).unwrap();
        let err = client
            .start("ws://127.0.0.1:1".into(), Arc::new(Recorder::default()))
            .unwrap_err();
        assert!(matches!(err, FfiError::Failed { .. }));
    }

    #[test]
    fn session_reports_reconnecting_when_relay_is_down_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let client = ClipFerryClient::open(
            dir.path().display().to_string(),
            Arc::new(MemVault::default()),
        )
        .unwrap();
        client.init_device("폰".into()).unwrap();
        client.create_group().unwrap();
        let events = Arc::new(Recorder::default());
        // 아무도 듣지 않는 포트라 연결이 안 된다 → 재연결 상태가 와야 한다.
        client
            .start("ws://127.0.0.1:1".into(), events.clone())
            .unwrap();
        assert!(client.is_running());
        let mut seen = false;
        for _ in 0..50 {
            if events
                .statuses
                .lock()
                .unwrap()
                .contains(&LinkStatus::Reconnecting)
            {
                seen = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(seen, "재연결 상태 알림이 오지 않았다");
        client.stop();
        assert!(!client.is_running());
    }
}
