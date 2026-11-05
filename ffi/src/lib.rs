//! 모바일 셸(Kotlin/Swift)용 UniFFI 바인딩.
//!
//! 코어(`clip-ferry-core`)의 비동기 세션을 셸이 쓰기 쉬운 동기 호출과 콜백으로 감싼다.
//! 암호·프로토콜 로직은 없다. tokio 런타임을 하나 쥐고, 코어를 부르고, 이벤트를 셸에 알릴 뿐이다.
//! F15: 기기·그룹 만들기, 상태 조회, 릴레이 세션 시작·중지, 연결 상태 알림.
//! F16: 페어링(초대·참여·승인), 기기 목록(온라인 표시), 그룹 나가기, QR 칸 만들기.
//! F17: 받은 텍스트·이미지를 셸에 넘긴다(셸이 클립보드에 쓴다).
//! 보내기(F18~)는 이후 기능에서 같은 방식으로 추가한다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clip_ferry_core::api::{Clip, ClipFerry, Config, Error, GroupHealth, ImageClip};
use clip_ferry_core::crypto::DeviceId;
use clip_ferry_core::pair_flow::{self, InviteFlow, InviteUpdate, JoinFlow, JoinUpdate};
use clip_ferry_core::pairing::Invite;
use clip_ferry_core::session::{ConnectionStatus, EventSink, Session};
use clip_ferry_core::store::{SecretStore, StoreError};
use clip_ferry_core::transfer;
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

impl From<pair_flow::StartError> for FfiError {
    fn from(e: pair_flow::StartError) -> Self {
        match e {
            pair_flow::StartError::App(e) => e.into(),
            pair_flow::StartError::Invite(e) => FfiError::Failed {
                reason: format!("초대 코드를 읽을 수 없습니다: {e}"),
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

/// 기기 목록 한 줄. 비밀값은 없다.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceEntry {
    /// 기기 id 앞 4바이트(hex).
    pub id_short: String,
    pub name: String,
    pub is_self: bool,
    /// 지금 그룹 채널에 붙어 있다 (릴레이 Presence). 연결이 끊겨 있으면 모두 `false`.
    pub online: bool,
}

/// 셸이 구현하는 알림 창구. 코어의 어느 스레드에서든 불릴 수 있다.
#[uniffi::export(with_foreign)]
pub trait CoreEvents: Send + Sync {
    fn on_status(&self, status: LinkStatus);
    /// 그룹 채널에 붙어 있는 기기 수 (이 기기 포함). 연결이 끊기면 0.
    /// 어느 기기인지는 `devices()`의 `online`으로 읽는다.
    fn on_online_count(&self, count: u32);
    /// 기기 목록·에포크·그룹 상태 중 무언가 바뀌었다. 화면은 `status()`·`devices()`로 다시 읽는다.
    fn on_changed(&self);
    /// 다른 기기가 보낸 텍스트(복호화·서명·중복 확인 끝). 비어 있지 않고 1 MiB 이하다.
    fn on_text(&self, from_name: String, text: String);
    /// 다른 기기가 보낸 이미지(길이·해시 확인 끝). `mime`은 보낸 쪽이 적은 값이 아니라
    /// 머리말로 다시 확인한 형식(`image/png`·`image/jpeg`·`image/gif`·`image/webp`)이다.
    fn on_image(&self, from_name: String, mime: String, bytes: Vec<u8>);
}

/// 코어 이벤트를 셸 콜백으로 옮긴다.
/// 온라인 기기 목록은 `devices()`가 쓰도록 함께 적어 둔다.
struct EventAdapter {
    events: Arc<dyn CoreEvents>,
    online: Arc<Mutex<Vec<DeviceId>>>,
}

impl EventSink for EventAdapter {
    fn on_clip(&self, clip: Clip) {
        self.events.on_text(clip.from_name, clip.text);
    }

    fn on_image(&self, image: ImageClip) {
        // 보낸 쪽이 적은 형식은 믿지 않는다. 머리말로 알 수 없는 형식이면 셸에 넘기지 않는다.
        let Some(sniffed) = transfer::sniff(&image.bytes) else {
            return;
        };
        self.events
            .on_image(image.from_name, sniffed.mime.to_string(), image.bytes);
    }

    fn on_status(&self, status: ConnectionStatus) {
        if status != ConnectionStatus::Connected {
            self.online.lock().unwrap().clear();
        }
        self.events.on_status(status.into());
    }

    fn on_devices_changed(&self) {
        self.events.on_changed();
    }

    fn on_epoch_changed(&self, _epoch: u64) {
        self.events.on_changed();
    }

    fn on_group_health(&self, _health: GroupHealth) {
        self.events.on_changed();
    }

    fn on_presence(&self, online: Vec<DeviceId>) {
        let count = online.len() as u32;
        *self.online.lock().unwrap() = online;
        self.events.on_online_count(count);
    }
}

/// 페어링 진행 알림. 초대하는 쪽과 참여하는 쪽이 같은 형태를 쓴다.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PairEvent {
    /// (참여) 릴레이에 연결했다. 초대한 기기의 응답을 기다린다.
    Connected,
    /// (초대) 참여 요청이 왔다. 상대 화면에 같은 `sas`가 떠 있는지 확인하고 `decide_invite`를 부른다.
    Request { peer_name: String, sas: String },
    /// (참여) 초대한 기기가 응답했다. 같은 `sas`가 그쪽에도 떠 있는지 확인하고 승인을 기다린다.
    WaitingApproval { peer_name: String, sas: String },
    /// (초대) 승인했고 로스터에 넣었다. `delivered`가 `false`면 상대가 환영 메시지를 못 받았을 수 있다.
    Approved { peer_name: String, delivered: bool },
    /// (참여) 그룹에 들어왔고 저장했다. 이제 `start`로 연결한다.
    Joined,
    /// (초대) 거절했다.
    Rejected,
    /// 만료·무효·오류로 끝났다. 사용자에게 보여 줄 문장.
    Ended { message: String },
}

/// 셸이 구현하는 페어링 알림 창구. 코어 스레드에서 불린다.
#[uniffi::export(with_foreign)]
pub trait PairListener: Send + Sync {
    fn on_pair_event(&self, event: PairEvent);
}

/// 시작한 초대. 화면에 보여 줄 값만 있다.
#[derive(Debug, Clone, uniffi::Record)]
pub struct InviteInfo {
    /// 사람이 입력하는 코드 (4자씩 `-`로 묶음).
    pub text_code: String,
    /// QR에 넣을 문자열.
    pub qr_string: String,
    /// 유닉스 시각(ms). 이때까지 참여해야 한다.
    pub expires_at_ms: u64,
}

/// QR 코드 칸. `dark[y * width + x]`가 `true`면 검은 칸이다. 여백(quiet zone)은 들어 있지 않다.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QrMatrix {
    pub width: u32,
    pub dark: Vec<bool>,
}

/// 진행 중인 페어링. 드롭하면 멈춘다.
enum Pairing {
    Invite(InviteFlow),
    Join(#[allow(dead_code)] JoinFlow),
}

fn short_id(id: &DeviceId) -> String {
    id.0[..4].iter().map(|b| format!("{b:02x}")).collect()
}

fn check_relay(url: &str) -> Result<&str, FfiError> {
    let url = url.trim();
    if url.starts_with("ws://") || url.starts_with("wss://") {
        Ok(url)
    } else {
        Err(FfiError::Failed {
            reason: "릴레이 주소는 ws:// 또는 wss://로 시작해야 합니다".into(),
        })
    }
}

/// 셸이 쥐는 코어 핸들. tokio 런타임과 (연결 중이면) 릴레이 세션을 함께 들고 있다.
#[derive(uniffi::Object)]
pub struct ClipFerryClient {
    app: Arc<ClipFerry>,
    runtime: tokio::runtime::Runtime,
    session: Mutex<Option<Session>>,
    online: Arc<Mutex<Vec<DeviceId>>>,
    pairing: Mutex<Option<Pairing>>,
    /// 페어링을 새로 시작하거나 닫을 때마다 늘린다. 이전 진행의 늦은 알림을 버리는 데 쓴다.
    pair_gen: Arc<AtomicU64>,
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
            online: Arc::new(Mutex::new(Vec::new())),
            pairing: Mutex::new(None),
            pair_gen: Arc::new(AtomicU64::new(0)),
        }))
    }

    pub fn is_initialized(&self) -> bool {
        self.app.is_initialized()
    }

    /// 기기 키와 로컬 저장 키를 만들고 이름을 정한다.
    pub fn init_device(&self, device_name: String) -> Result<(), FfiError> {
        Ok(self.app.init_device(device_name.trim())?)
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

    /// 그룹 구성원 목록. 그룹이 없으면 비어 있다.
    pub fn devices(&self) -> Vec<DeviceEntry> {
        let online = self.online.lock().unwrap().clone();
        self.app
            .devices()
            .into_iter()
            .map(|d| DeviceEntry {
                id_short: short_id(&d.device_id),
                online: online.contains(&d.device_id),
                name: d.name,
                is_self: d.is_self,
            })
            .collect()
    }

    /// 그룹에서 나가 그룹이 없는 기기로 돌아간다(제거되었거나 다른 그룹에 참여하려고).
    /// 연결과 진행 중인 페어링도 닫는다. 기기 키와 이름은 그대로다.
    pub fn leave_group(&self) -> Result<(), FfiError> {
        self.stop();
        self.cancel_pairing();
        Ok(self.app.leave_group()?)
    }

    /// 릴레이에 (다시) 연결하는 세션을 시작한다. 이미 있으면 닫고 새로 연다.
    /// 연결 결과는 `events`로 온다.
    pub fn start(&self, relay_url: String, events: Arc<dyn CoreEvents>) -> Result<(), FfiError> {
        let url = check_relay(&relay_url)?;
        let _guard = self.runtime.enter();
        let mut slot = self.session.lock().unwrap();
        *slot = None;
        self.online.lock().unwrap().clear();
        let session = self.app.start_session(
            url,
            TransportConfig::default(),
            Arc::new(EventAdapter {
                events,
                online: self.online.clone(),
            }),
        )?;
        *slot = Some(session);
        Ok(())
    }

    /// 세션을 닫는다. 연결 중이 아니면 아무 일도 하지 않는다.
    pub fn stop(&self) {
        let _guard = self.runtime.enter();
        *self.session.lock().unwrap() = None;
        self.online.lock().unwrap().clear();
    }

    pub fn is_running(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }

    /// 이 그룹에 다른 기기를 초대한다. QR에는 `relay_url`이 들어가 참여하는 쪽이 같은 릴레이를 쓴다.
    /// 진행은 `listener`로 온다. 이미 진행 중인 페어링은 닫는다.
    pub fn start_invite(
        &self,
        relay_url: String,
        listener: Arc<dyn PairListener>,
    ) -> Result<InviteInfo, FfiError> {
        let url = check_relay(&relay_url)?;
        let _guard = self.runtime.enter();
        let mut slot = self.pairing.lock().unwrap();
        *slot = None;
        let forward = self.forwarder(listener);
        let flow = pair_flow::start_invite(&self.app, url, TransportConfig::default(), move |u| {
            forward(match u {
                InviteUpdate::Request { joiner_name, sas } => PairEvent::Request {
                    peer_name: joiner_name,
                    sas,
                },
                InviteUpdate::Approved {
                    joiner_name,
                    delivered,
                } => PairEvent::Approved {
                    peer_name: joiner_name,
                    delivered,
                },
                InviteUpdate::Rejected => PairEvent::Rejected,
                InviteUpdate::Ended(message) => PairEvent::Ended { message },
            })
        })?;
        let info = InviteInfo {
            text_code: flow.text_code.clone(),
            qr_string: flow.qr_string.clone(),
            expires_at_ms: flow.expires_at_ms,
        };
        *slot = Some(Pairing::Invite(flow));
        Ok(info)
    }

    /// 초대 요청을 승인(`true`)하거나 거절(`false`)한다. 진행 중인 초대가 없으면 무시된다.
    pub fn decide_invite(&self, approve: bool) {
        if let Some(Pairing::Invite(flow)) = self.pairing.lock().unwrap().as_ref() {
            flow.decide(approve);
        }
    }

    /// 초대 코드(QR 문자열 또는 텍스트 코드)로 참여를 시작한다. 그룹이 없는 기기여야 한다.
    /// 릴레이는 QR에 들어 있는 주소, 없으면(텍스트 코드) `default_relay`를 쓴다.
    /// 돌려주는 값은 실제로 쓰는 릴레이 주소다. 참여가 끝나면 셸은 이 주소로 연결한다.
    pub fn start_join(
        &self,
        code: String,
        default_relay: String,
        listener: Arc<dyn PairListener>,
    ) -> Result<String, FfiError> {
        let invite = Invite::parse(&code).map_err(pair_flow::StartError::from)?;
        let relay = match invite.relay() {
            Some(r) => check_relay(r)?.to_string(),
            None => check_relay(&default_relay)?.to_string(),
        };
        let _guard = self.runtime.enter();
        let mut slot = self.pairing.lock().unwrap();
        *slot = None;
        let forward = self.forwarder(listener);
        let flow = pair_flow::start_join(
            &self.app,
            &code,
            Some(relay.clone()),
            &relay,
            TransportConfig::default(),
            move |u| {
                forward(match u {
                    JoinUpdate::Connected => PairEvent::Connected,
                    JoinUpdate::WaitingApproval { inviter_name, sas } => {
                        PairEvent::WaitingApproval {
                            peer_name: inviter_name,
                            sas,
                        }
                    }
                    JoinUpdate::Joined => PairEvent::Joined,
                    JoinUpdate::Ended(message) => PairEvent::Ended { message },
                })
            },
        )?;
        *slot = Some(Pairing::Join(flow));
        Ok(relay)
    }

    /// 진행 중이거나 끝난 페어링을 닫는다(진행 중이면 취소). 이후 그 진행의 알림은 오지 않는다.
    pub fn cancel_pairing(&self) {
        let _guard = self.runtime.enter();
        self.pair_gen.fetch_add(1, Ordering::SeqCst);
        *self.pairing.lock().unwrap() = None;
    }
}

impl ClipFerryClient {
    /// 새 페어링 번호를 받고, 그 번호가 아직 현재일 때만 알림을 넘기는 함수를 만든다.
    fn forwarder(&self, listener: Arc<dyn PairListener>) -> impl Fn(PairEvent) + Send + Sync {
        let gen = self.pair_gen.clone();
        let mine = gen.fetch_add(1, Ordering::SeqCst) + 1;
        move |event| {
            if gen.load(Ordering::SeqCst) == mine {
                listener.on_pair_event(event);
            }
        }
    }
}

/// QR 코드 칸을 만든다. 데이터가 너무 길면 `None`.
#[uniffi::export]
pub fn qr_matrix(data: String) -> Option<QrMatrix> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some(QrMatrix {
        width: width as u32,
        dark,
    })
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
    use std::time::Duration;

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
        online: Mutex<Vec<u32>>,
        texts: Mutex<Vec<(String, String)>>,
        images: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    impl CoreEvents for Recorder {
        fn on_status(&self, status: LinkStatus) {
            self.statuses.lock().unwrap().push(status);
        }
        fn on_online_count(&self, count: u32) {
            self.online.lock().unwrap().push(count);
        }
        fn on_changed(&self) {}
        fn on_text(&self, from_name: String, text: String) {
            self.texts.lock().unwrap().push((from_name, text));
        }
        fn on_image(&self, from_name: String, mime: String, bytes: Vec<u8>) {
            self.images.lock().unwrap().push((from_name, mime, bytes));
        }
    }

    #[derive(Default)]
    struct PairRecorder(Mutex<Vec<PairEvent>>);

    impl PairListener for PairRecorder {
        fn on_pair_event(&self, event: PairEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl PairRecorder {
        /// `pick`이 값을 돌려줄 때까지 기다린다 (최대 10초).
        fn wait<T>(&self, mut pick: impl FnMut(&PairEvent) -> Option<T>) -> T {
            for _ in 0..200 {
                if let Some(v) = self.0.lock().unwrap().iter().find_map(&mut pick) {
                    return v;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            panic!(
                "기다린 페어링 알림이 오지 않았다: {:?}",
                self.0.lock().unwrap()
            );
        }
    }

    fn wait_until(mut ok: impl FnMut() -> bool, what: &str) {
        for _ in 0..200 {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("{what}");
    }

    struct Phone {
        client: Arc<ClipFerryClient>,
        _dir: tempfile::TempDir,
    }

    fn phone(name: &str, group: bool) -> Phone {
        let dir = tempfile::tempdir().unwrap();
        let client = ClipFerryClient::open(
            dir.path().display().to_string(),
            Arc::new(MemVault::default()),
        )
        .unwrap();
        client.init_device(name.into()).unwrap();
        if group {
            client.create_group().unwrap();
        }
        Phone { client, _dir: dir }
    }

    /// 테스트용 릴레이를 따로 만든 런타임에서 띄운다.
    fn relay() -> (
        tokio::runtime::Runtime,
        clip_ferry_relay::RelayHandle,
        String,
    ) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let handle = rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            clip_ferry_relay::spawn(
                listener,
                clip_ferry_relay::Config {
                    max_frames_per_sec: 1000,
                    ..clip_ferry_relay::Config::default()
                },
            )
        });
        let url = format!("ws://{}", handle.local_addr());
        (rt, handle, url)
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
        let p = phone("폰", true);
        let err = p
            .client
            .start(
                "http://127.0.0.1:8787".into(),
                Arc::new(Recorder::default()),
            )
            .unwrap_err();
        assert!(matches!(err, FfiError::Failed { .. }));
        assert!(!p.client.is_running());
        let err = p
            .client
            .start_invite("http://x".into(), Arc::new(PairRecorder::default()))
            .unwrap_err();
        assert!(matches!(err, FfiError::Failed { .. }));
    }

    #[test]
    fn start_without_group_fails() {
        let p = phone("폰", false);
        let err = p
            .client
            .start("ws://127.0.0.1:1".into(), Arc::new(Recorder::default()))
            .unwrap_err();
        assert!(matches!(err, FfiError::Failed { .. }));
        assert!(p.client.devices().is_empty());
    }

    #[test]
    fn session_reports_reconnecting_when_relay_is_down_and_stops() {
        let p = phone("폰", true);
        let events = Arc::new(Recorder::default());
        // 아무도 듣지 않는 포트라 연결이 안 된다 → 재연결 상태가 와야 한다.
        p.client
            .start("ws://127.0.0.1:1".into(), events.clone())
            .unwrap();
        assert!(p.client.is_running());
        wait_until(
            || {
                events
                    .statuses
                    .lock()
                    .unwrap()
                    .contains(&LinkStatus::Reconnecting)
            },
            "재연결 상태 알림이 오지 않았다",
        );
        p.client.stop();
        assert!(!p.client.is_running());
    }

    #[test]
    fn qr_matrix_is_square() {
        let qr = qr_matrix("clipferry://pair/1/abc?relay=wss%3A%2F%2Fexample".into()).unwrap();
        assert!(qr.width >= 21);
        assert_eq!(qr.dark.len(), (qr.width * qr.width) as usize);
        assert!(qr.dark.iter().any(|d| *d));
        // 왼쪽 위 찾기 패턴의 모서리는 항상 검다.
        assert!(qr.dark[0]);
    }

    #[test]
    fn join_rejects_bad_code_and_existing_group() {
        let lone = phone("폰", false);
        let err = lone
            .client
            .start_join(
                "ABCD".into(),
                "ws://127.0.0.1:1".into(),
                Arc::new(PairRecorder::default()),
            )
            .unwrap_err();
        assert!(matches!(err, FfiError::Failed { .. }));

        let inviter = phone("PC", true);
        let info = inviter
            .client
            .start_invite("ws://127.0.0.1:1".into(), Arc::new(PairRecorder::default()))
            .unwrap();
        let grouped = phone("폰2", true);
        let err = grouped
            .client
            .start_join(
                info.text_code,
                "ws://127.0.0.1:1".into(),
                Arc::new(PairRecorder::default()),
            )
            .unwrap_err();
        assert!(matches!(err, FfiError::Failed { .. }));
    }

    /// PC가 초대하고 폰이 QR 문자열로 참여 → 같은 확인 숫자 → 승인 → 양쪽 목록에 서로 보이고 온라인.
    #[test]
    fn pairs_by_qr_then_both_see_each_other_online() {
        let (_rt, _relay, url) = relay();
        let pc = phone("PC", true);
        let mobile = phone("폰", false);

        let pc_pair = Arc::new(PairRecorder::default());
        let info = pc
            .client
            .start_invite(url.clone(), pc_pair.clone())
            .unwrap();
        assert!(info.qr_string.starts_with("clipferry://pair/"));
        assert!(info.expires_at_ms > 0);

        let phone_pair = Arc::new(PairRecorder::default());
        // QR에는 릴레이 주소가 들어 있어 기본값(엉뚱한 주소)을 쓰지 않는다.
        let used = mobile
            .client
            .start_join(
                info.qr_string.clone(),
                "ws://127.0.0.1:1".into(),
                phone_pair.clone(),
            )
            .unwrap();
        assert_eq!(used, url);

        let pc_sas = pc_pair.wait(|e| match e {
            PairEvent::Request { peer_name, sas } => {
                assert_eq!(peer_name, "폰");
                Some(sas.clone())
            }
            _ => None,
        });
        let phone_sas = phone_pair.wait(|e| match e {
            PairEvent::WaitingApproval { peer_name, sas } => {
                assert_eq!(peer_name, "PC");
                Some(sas.clone())
            }
            _ => None,
        });
        assert_eq!(pc_sas, phone_sas);
        assert_eq!(pc_sas.len(), 6);

        pc.client.decide_invite(true);
        pc_pair.wait(|e| {
            matches!(
                e,
                PairEvent::Approved {
                    delivered: true,
                    ..
                }
            )
            .then_some(())
        });
        phone_pair.wait(|e| matches!(e, PairEvent::Joined).then_some(()));
        assert!(mobile.client.has_group());
        assert_eq!(mobile.client.status().unwrap().member_count, 2);
        assert_eq!(pc.client.status().unwrap().member_count, 2);
        pc.client.cancel_pairing();
        mobile.client.cancel_pairing();

        // 양쪽이 연결하면 서로 온라인으로 보인다.
        let pc_events = Arc::new(Recorder::default());
        let phone_events = Arc::new(Recorder::default());
        pc.client.start(url.clone(), pc_events).unwrap();
        mobile.client.start(used, phone_events).unwrap();
        for c in [&pc.client, &mobile.client] {
            wait_until(
                || {
                    let list = c.devices();
                    list.len() == 2 && list.iter().all(|d| d.online)
                },
                "두 기기가 서로 온라인으로 보이지 않았다",
            );
        }
        let names: Vec<String> = mobile
            .client
            .devices()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(names.contains(&"PC".to_string()) && names.contains(&"폰".to_string()));
        assert_eq!(
            mobile.client.devices().iter().filter(|d| d.is_self).count(),
            1
        );

        // 연결을 끊으면 온라인 표시가 모두 꺼진다.
        mobile.client.stop();
        assert!(mobile.client.devices().iter().all(|d| !d.online));
    }

    /// 폰이 초대하고 PC가 텍스트 코드로 참여, 폰이 거절하면 PC는 실패로 끝난다.
    #[test]
    fn phone_invites_text_code_and_rejects() {
        let (_rt, _relay, url) = relay();
        let mobile = phone("폰", true);
        let pc = phone("PC", false);

        let phone_pair = Arc::new(PairRecorder::default());
        let info = mobile
            .client
            .start_invite(url.clone(), phone_pair.clone())
            .unwrap();
        let pc_pair = Arc::new(PairRecorder::default());
        // 텍스트 코드에는 릴레이가 없어 기본값을 쓴다.
        let used = pc
            .client
            .start_join(info.text_code.clone(), url.clone(), pc_pair.clone())
            .unwrap();
        assert_eq!(used, url);

        phone_pair.wait(|e| matches!(e, PairEvent::Request { .. }).then_some(()));
        mobile.client.decide_invite(false);
        phone_pair.wait(|e| matches!(e, PairEvent::Rejected).then_some(()));
        pc_pair.wait(|e| matches!(e, PairEvent::Ended { .. }).then_some(()));
        assert!(!pc.client.has_group());
        assert_eq!(mobile.client.status().unwrap().member_count, 1);
    }

    /// 취소한 페어링의 늦은 알림은 오지 않는다.
    #[test]
    fn cancelled_pairing_sends_no_more_events() {
        let (_rt, _relay, url) = relay();
        let pc = phone("PC", true);
        let mobile = phone("폰", false);
        let pc_pair = Arc::new(PairRecorder::default());
        let info = pc
            .client
            .start_invite(url.clone(), pc_pair.clone())
            .unwrap();
        pc.client.cancel_pairing();

        let phone_pair = Arc::new(PairRecorder::default());
        mobile
            .client
            .start_join(info.qr_string, url, phone_pair.clone())
            .unwrap();
        phone_pair.wait(|e| matches!(e, PairEvent::Connected).then_some(()));
        std::thread::sleep(Duration::from_millis(500));
        assert!(pc_pair.0.lock().unwrap().is_empty());
    }

    /// `pc`가 초대하고 `mobile`이 QR로 참여해 승인까지 끝낸다.
    fn pair(pc: &Phone, mobile: &Phone, url: &str) {
        let pc_pair = Arc::new(PairRecorder::default());
        let info = pc
            .client
            .start_invite(url.to_string(), pc_pair.clone())
            .unwrap();
        let phone_pair = Arc::new(PairRecorder::default());
        mobile
            .client
            .start_join(info.qr_string, url.to_string(), phone_pair.clone())
            .unwrap();
        pc_pair.wait(|e| matches!(e, PairEvent::Request { .. }).then_some(()));
        pc.client.decide_invite(true);
        phone_pair.wait(|e| matches!(e, PairEvent::Joined).then_some(()));
        pc.client.cancel_pairing();
        mobile.client.cancel_pairing();
    }

    /// 머리말만 갖춘 PNG (sniff가 형식과 크기를 읽을 수 있다).
    fn tiny_png(width: u32, height: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b.resize(300_000, 7); // 청크 두 개로 나뉘는 크기
        b
    }

    /// 다른 기기가 보낸 텍스트·이미지가 셸 콜백으로 온다. 형식을 알 수 없는 이미지는 오지 않는다.
    #[test]
    fn received_text_and_image_reach_the_shell() {
        let (_rt, _relay, url) = relay();
        let pc = phone("PC", true);
        let mobile = phone("폰", false);
        pair(&pc, &mobile, &url);

        let pc_events = Arc::new(Recorder::default());
        let phone_events = Arc::new(Recorder::default());
        pc.client.start(url.clone(), pc_events).unwrap();
        mobile.client.start(url, phone_events.clone()).unwrap();
        for c in [&pc.client, &mobile.client] {
            wait_until(
                || c.devices().iter().filter(|d| d.online).count() == 2,
                "두 기기가 서로 온라인으로 보이지 않았다",
            );
        }

        let send = |f: &dyn Fn(&Session) -> u32| {
            let slot = pc.client.session.lock().unwrap();
            f(slot.as_ref().unwrap())
        };
        let rt = &pc.client.runtime;
        assert_eq!(
            send(&|s| rt.block_on(s.send_text("안녕 폰 📋")).unwrap()),
            1
        );
        wait_until(
            || !phone_events.texts.lock().unwrap().is_empty(),
            "텍스트가 오지 않았다",
        );
        assert_eq!(
            phone_events.texts.lock().unwrap()[0],
            ("PC".to_string(), "안녕 폰 📋".to_string())
        );

        // 보낸 쪽은 JPEG라고 적었지만 실제로는 PNG다. 셸에는 머리말로 확인한 형식이 간다.
        let png = tiny_png(3, 2);
        send(&|s| rt.block_on(s.send_image(&png, "image/jpeg", 3, 2)).unwrap());
        wait_until(
            || !phone_events.images.lock().unwrap().is_empty(),
            "이미지가 오지 않았다",
        );
        {
            let images = phone_events.images.lock().unwrap();
            assert_eq!(images[0].0, "PC");
            assert_eq!(images[0].1, "image/png");
            assert_eq!(images[0].2, png);
        }

        // 형식을 알 수 없는 바이트는 셸에 넘기지 않는다. 뒤이은 텍스트로 처리가 끝났음을 확인한다.
        send(&|s| {
            rt.block_on(s.send_image(&[1u8; 1000], "image/png", 1, 1))
                .unwrap()
        });
        send(&|s| rt.block_on(s.send_text("끝")).unwrap());
        wait_until(
            || phone_events.texts.lock().unwrap().len() == 2,
            "두 번째 텍스트가 오지 않았다",
        );
        assert_eq!(phone_events.images.lock().unwrap().len(), 1);
        // 자기가 보낸 것은 자기에게 오지 않는다.
        assert!(phone_events
            .texts
            .lock()
            .unwrap()
            .iter()
            .all(|(n, _)| n == "PC"));
    }

    #[test]
    fn leave_group_returns_to_setup() {
        let p = phone("폰", true);
        p.client
            .start("ws://127.0.0.1:1".into(), Arc::new(Recorder::default()))
            .unwrap();
        let before = p.client.status().unwrap().device_id_short;
        p.client.leave_group().unwrap();
        assert!(!p.client.is_running());
        assert!(!p.client.has_group());
        let s = p.client.status().unwrap();
        assert_eq!(s.epoch, None);
        assert_eq!(s.device_id_short, before);
        assert!(p.client.devices().is_empty());
        // 다시 새 그룹을 만들 수 있다.
        p.client.create_group().unwrap();
        assert!(p.client.has_group());
    }
}
