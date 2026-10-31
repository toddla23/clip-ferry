//! 코어와 화면(트레이·창)을 이어 주는 얇은 층.
//! 암호·프로토콜 로직은 없다. 코어를 열고, 세션과 페어링을 시작하고, 상태를 화면에 알린다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use clip_ferry_core::api::{Clip, ClipFerry, Config, Error, GroupHealth, ImageClip};
use clip_ferry_core::pair_flow::{self, InviteFlow, InviteUpdate, JoinFlow, JoinUpdate};
use clip_ferry_core::session::{ConnectionStatus, EventSink, Session};
use clip_ferry_core::store::SecretStore;
use clip_ferry_core::transfer::MAX_IMAGE_BYTES;
use clip_ferry_core::transport::TransportConfig;
use tauri::{AppHandle, Emitter};

use crate::clipsync::{ClipSync, Content};
use crate::imgcodec;
use crate::settings::{self, Settings};
use crate::sysclip::SystemClipboard;
use crate::tray;
use crate::view::{self, AppView, Connection, PairKind, PairPhase, PairingView};

/// 진행 중인 페어링 작업. 드롭하면 멈춘다.
enum Flow {
    Invite(InviteFlow),
    // 읽지 않지만, 드롭할 때 참여 작업이 멈추도록 쥐고 있다.
    Join(#[allow(dead_code)] JoinFlow),
}

struct Pairing {
    /// 이전 진행의 늦은 알림이 새 진행을 건드리지 못하게 구분한다.
    id: u64,
    view: PairingView,
    flow: Option<Flow>,
}

struct Inner {
    relay_url: String,
    connection: Connection,
    error: Option<String>,
    session: Option<Arc<Session>>,
    pairing: Option<Pairing>,
    next_pairing_id: u64,
}

pub struct Shell {
    app: Arc<ClipFerry>,
    clip: Arc<ClipSync<SystemClipboard>>,
    sync_sensitive: bool,
    handle: AppHandle,
    data_dir: PathBuf,
    seq: AtomicU64,
    inner: Mutex<Inner>,
}

/// 코어 이벤트를 받아 화면을 갱신한다. 셸이 사라져도 안전하도록 약한 참조만 쥔다.
struct Sink(Weak<Shell>);

impl EventSink for Sink {
    fn on_clip(&self, clip: Clip) {
        // 받은 텍스트는 클립보드에만 쓴다. 디스크나 화면에는 남기지 않는다.
        if let Some(shell) = self.0.upgrade() {
            if let Err(reason) = shell.clip.write_received(&Content::Text(clip.text)) {
                shell.notify(&format!(
                    "{}에서 받은 텍스트를 클립보드에 쓰지 못했습니다 ({reason})",
                    clip.from_name
                ));
            }
        }
    }

    fn on_image(&self, image: ImageClip) {
        // 디코딩은 무거우므로 따로 돌린다. 받은 이미지를 열거나 쓰지 못하면 알림으로 알린다.
        if let Some(shell) = self.0.upgrade() {
            tauri::async_runtime::spawn_blocking(move || {
                let from = &image.from_name;
                match imgcodec::decode(&image.bytes) {
                    Err(_) => shell.notify(&format!(
                        "{from}에서 받은 이미지를 열 수 없어 버렸습니다 (깨졌거나 너무 큰 이미지)"
                    )),
                    Ok(img) => {
                        if let Err(reason) = shell.clip.write_received(&Content::Image(img)) {
                            shell.notify(&format!(
                                "{from}에서 받은 이미지를 클립보드에 쓰지 못했습니다 ({reason})"
                            ));
                        }
                    }
                }
            });
        }
    }

    fn on_status(&self, status: ConnectionStatus) {
        if let Some(shell) = self.0.upgrade() {
            shell.inner.lock().unwrap().connection = status.into();
            shell.publish();
        }
    }

    fn on_devices_changed(&self) {
        self.refresh();
    }

    fn on_epoch_changed(&self, _epoch: u64) {
        self.refresh();
    }

    fn on_group_health(&self, _health: GroupHealth) {
        self.refresh();
    }
}

impl Sink {
    fn refresh(&self) {
        if let Some(shell) = self.0.upgrade() {
            shell.publish();
        }
    }
}

impl Shell {
    pub fn new(
        handle: AppHandle,
        data_dir: PathBuf,
        secrets: Box<dyn SecretStore>,
    ) -> Result<Arc<Self>, Error> {
        std::fs::create_dir_all(&data_dir).map_err(|_| Error::Io)?;
        let app = Arc::new(ClipFerry::open(
            Config {
                data_dir: data_dir.clone(),
            },
            secrets,
        )?);
        let Settings {
            relay_url,
            sync_sensitive,
        } = settings::load(&data_dir);
        Ok(Arc::new(Self {
            app,
            clip: ClipSync::new(SystemClipboard::new()),
            sync_sensitive,
            handle,
            data_dir,
            seq: AtomicU64::new(1),
            inner: Mutex::new(Inner {
                relay_url,
                connection: Connection::Reconnecting,
                error: None,
                session: None,
                pairing: None,
                next_pairing_id: 1,
            }),
        }))
    }

    /// 앱이 켜질 때 한 번. 그룹이 있으면 릴레이에 연결하고, 없으면 첫 실행 마법사를 띄운다.
    /// tokio 런타임 안에서 불러야 한다.
    pub fn begin(self: &Arc<Self>) {
        self.watch_clipboard();
        match self.app.status() {
            Ok(s) if s.epoch.is_some() => self.start(),
            _ => self.publish(),
        }
    }

    /// 클립보드 감시를 시작한다(앱이 켜져 있는 동안 계속). 연결된 세션이 없으면 내용을 읽지 않는다.
    fn watch_clipboard(self: &Arc<Self>) {
        let (on, send) = (Arc::downgrade(self), Arc::downgrade(self));
        let clip = self.clip.clone();
        tauri::async_runtime::spawn(clip.watch(
            move || on.upgrade().is_some_and(|s| s.has_session()),
            !self.sync_sensitive,
            move |content| {
                let shell = send.upgrade();
                async move {
                    match shell {
                        Some(shell) => shell.send_clip(content).await,
                        None => false,
                    }
                }
            },
        ));
    }

    fn has_session(&self) -> bool {
        self.inner.lock().unwrap().session.is_some()
    }

    /// 복사한 내용을 그룹에 보낸다. 다시 시도할 필요가 없으면(보냈거나, 일부러 안 보냈으면) `true`.
    async fn send_clip(&self, content: Content) -> bool {
        let session = self.inner.lock().unwrap().session.clone();
        let Some(session) = session else {
            return false;
        };
        match content {
            Content::Text(text) => session.send_text(&text).await.is_ok(),
            Content::Image(img) => {
                let (width, height) = (img.width as u32, img.height as u32);
                // PNG 인코딩은 무거우므로 따로 돌린다.
                let png =
                    tauri::async_runtime::spawn_blocking(move || imgcodec::encode_png(&img)).await;
                let Ok(Ok(png)) = png else {
                    return false;
                };
                if png.len() > MAX_IMAGE_BYTES {
                    self.notify(&format!(
                        "복사한 이미지가 {:.1} MiB라 20 MiB를 넘어 보내지 않았습니다",
                        png.len() as f64 / (1024.0 * 1024.0)
                    ));
                    return true;
                }
                session
                    .send_image(&png, "image/png", width, height)
                    .await
                    .is_ok()
            }
        }
    }

    /// 시스템 알림을 띄운다. 실패해도 무시한다.
    fn notify(&self, body: &str) {
        use tauri_plugin_notification::NotificationExt;
        let _ = self
            .handle
            .notification()
            .builder()
            .title("clip-ferry")
            .body(body)
            .show();
    }

    /// 릴레이 세션을 (다시) 시작한다. tokio 런타임 안에서 불러야 한다.
    pub fn start(self: &Arc<Self>) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.session = None;
            let sink = Arc::new(Sink(Arc::downgrade(self)));
            match self
                .app
                .start_session(&inner.relay_url, TransportConfig::default(), sink)
            {
                Ok(session) => {
                    inner.session = Some(Arc::new(session));
                    inner.connection = Connection::Reconnecting;
                    inner.error = None;
                }
                // 제거된 기기는 화면의 안내 문구로 충분하다.
                Err(Error::Removed) => {
                    inner.connection = Connection::Stopped;
                    inner.error = None;
                }
                Err(e) => {
                    inner.connection = Connection::Stopped;
                    inner.error = Some(e.to_string());
                }
            }
        }
        self.publish();
    }

    pub fn set_relay_url(self: &Arc<Self>, input: &str) -> Result<AppView, String> {
        let relay_url = settings::validate_relay_url(input)?;
        settings::save(
            &self.data_dir,
            &Settings {
                relay_url: relay_url.clone(),
                sync_sensitive: self.sync_sensitive,
            },
        )
        .map_err(|_| "설정을 저장하지 못했습니다".to_string())?;
        self.inner.lock().unwrap().relay_url = relay_url;
        // 그룹이 아직 없으면 연결할 것이 없다.
        if self.app.connection().is_some() {
            self.start();
        } else {
            self.publish();
        }
        Ok(self.view())
    }

    // ------------------------------------------------------------ 첫 실행

    /// 기기가 아직 없으면 이름을 정해 만든다. 이미 있으면 그대로 둔다.
    fn ensure_device(&self, name: &str) -> Result<(), String> {
        if !self.app.is_initialized() {
            self.app
                .init_device(name.trim())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// 새 그룹을 만들고 릴레이에 연결한다.
    pub fn create_group(self: &Arc<Self>, name: &str) -> Result<AppView, String> {
        self.ensure_device(name)?;
        if self
            .app
            .status()
            .map_err(|e| e.to_string())?
            .epoch
            .is_none()
        {
            self.app.create_group().map_err(|e| e.to_string())?;
        }
        self.start();
        Ok(self.view())
    }

    // ------------------------------------------------------------ 페어링

    fn new_pairing_id(&self) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_pairing_id;
        inner.next_pairing_id += 1;
        id
    }

    /// 진행 상태를 바꾼다. `id`가 지금 진행과 다르면(늦은 알림) 무시하고 `false`.
    fn update_pairing(&self, id: u64, f: impl FnOnce(&mut PairingView)) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match inner.pairing.as_mut() {
            Some(p) if p.id == id => {
                f(&mut p.view);
                true
            }
            _ => false,
        }
    }

    /// 초대 코드로 다른 그룹에 참여한다.
    pub fn start_join(self: &Arc<Self>, name: &str, code: &str) -> Result<AppView, String> {
        self.ensure_device(name)?;
        let id = self.new_pairing_id();
        let relay = self.inner.lock().unwrap().relay_url.clone();
        // 콜백이 먼저 와도 받을 수 있게 진행 상태를 먼저 만들어 둔다.
        let old = self.inner.lock().unwrap().pairing.replace(Pairing {
            id,
            view: PairingView::new(PairKind::Join, PairPhase::Connecting),
            flow: None,
        });
        drop(old);

        let weak = Arc::downgrade(self);
        let flow = pair_flow::start_join(
            &self.app,
            code,
            None,
            &relay,
            TransportConfig::default(),
            move |update| {
                if let Some(shell) = weak.upgrade() {
                    shell.on_join_update(id, update);
                }
            },
        );
        match flow {
            Ok(flow) => {
                if let Some(p) = self.inner.lock().unwrap().pairing.as_mut() {
                    if p.id == id {
                        p.flow = Some(Flow::Join(flow));
                    }
                }
                self.publish();
                Ok(self.view())
            }
            Err(e) => {
                self.take_pairing(id);
                self.publish();
                Err(e.to_string())
            }
        }
    }

    fn on_join_update(self: &Arc<Self>, id: u64, update: JoinUpdate) {
        match update {
            JoinUpdate::Connected => {
                self.update_pairing(id, |v| {
                    v.message = Some("초대한 기기의 응답을 기다리는 중입니다".into());
                });
            }
            JoinUpdate::WaitingApproval { inviter_name, sas } => {
                self.update_pairing(id, |v| {
                    v.phase = PairPhase::Waiting;
                    v.peer_name = Some(inviter_name);
                    v.sas = Some(sas);
                    v.message = None;
                });
            }
            JoinUpdate::Joined => {
                // 그룹에 들어왔다. 진행 화면을 닫고 릴레이에 연결한다.
                if self.take_pairing(id).is_some() {
                    self.start();
                    return;
                }
            }
            JoinUpdate::Ended(message) => {
                self.update_pairing(id, |v| {
                    v.phase = PairPhase::Failed;
                    v.message = Some(message);
                });
            }
        }
        self.publish();
    }

    /// 다른 기기를 내 그룹에 초대한다.
    pub fn start_invite(self: &Arc<Self>) -> Result<AppView, String> {
        if self.app.connection().is_none() {
            return Err("먼저 그룹이 있어야 합니다".into());
        }
        let id = self.new_pairing_id();
        let relay = self.inner.lock().unwrap().relay_url.clone();
        let weak = Arc::downgrade(self);
        let flow = pair_flow::start_invite(
            &self.app,
            &relay,
            TransportConfig::default(),
            move |update| {
                if let Some(shell) = weak.upgrade() {
                    shell.on_invite_update(id, update);
                }
            },
        )
        .map_err(|e| e.to_string())?;

        let mut pv = PairingView::new(PairKind::Invite, PairPhase::Waiting);
        pv.text_code = Some(flow.text_code.clone());
        pv.qr = view::qr(&flow.qr_string);
        pv.expires_at_ms = Some(flow.expires_at_ms);
        let old = self.inner.lock().unwrap().pairing.replace(Pairing {
            id,
            view: pv,
            flow: Some(Flow::Invite(flow)),
        });
        drop(old);
        self.publish();
        Ok(self.view())
    }

    fn on_invite_update(&self, id: u64, update: InviteUpdate) {
        let changed = self.update_pairing(id, |v| match update {
            InviteUpdate::Request { joiner_name, sas } => {
                v.phase = PairPhase::Request;
                v.peer_name = Some(joiner_name);
                v.sas = Some(sas);
            }
            InviteUpdate::Approved {
                joiner_name,
                delivered,
            } => {
                v.phase = PairPhase::Done;
                v.peer_name = Some(joiner_name);
                v.message = (!delivered).then(|| {
                    "승인했지만 상대 기기가 응답을 받지 못했을 수 있습니다. 상대 앱을 확인하고, 안 되면 기기를 제거한 뒤 다시 초대하세요".to_string()
                });
            }
            InviteUpdate::Rejected => {
                v.phase = PairPhase::Failed;
                v.message = Some("참여 요청을 거절했습니다".into());
            }
            InviteUpdate::Ended(message) => {
                v.phase = PairPhase::Failed;
                v.message = Some(message);
            }
        });
        if changed {
            self.publish();
        }
    }

    /// 참여 요청을 승인(`true`)하거나 거절(`false`)한다.
    pub fn decide_invite(&self, approve: bool) {
        let inner = self.inner.lock().unwrap();
        if let Some(Pairing {
            flow: Some(Flow::Invite(flow)),
            ..
        }) = inner.pairing.as_ref()
        {
            flow.decide(approve);
        }
    }

    fn take_pairing(&self, id: u64) -> Option<Pairing> {
        let mut inner = self.inner.lock().unwrap();
        if inner.pairing.as_ref().is_some_and(|p| p.id == id) {
            inner.pairing.take()
        } else {
            None
        }
    }

    /// 진행 중이거나 끝난 페어링 화면을 닫는다(진행 중이면 취소).
    pub fn close_pairing(&self) -> AppView {
        let old = self.inner.lock().unwrap().pairing.take();
        drop(old);
        self.publish();
        self.view()
    }

    // ------------------------------------------------------------ 화면

    pub fn view(&self) -> AppView {
        let (connection, relay_url, error, pairing) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.connection,
                inner.relay_url.clone(),
                inner.error.clone(),
                inner.pairing.as_ref().map(|p| p.view.clone()),
            )
        };
        let mut view = view::build(&self.app, connection, &relay_url, error);
        view.pairing = pairing;
        view.default_name = default_device_name();
        view.seq = self.seq.fetch_add(1, Ordering::SeqCst);
        view
    }

    /// 트레이와 창에 현재 상태를 알린다.
    fn publish(&self) {
        let view = self.view();
        tray::update(&self.handle, &view);
        let _ = self.handle.emit("view-changed", &view);
    }
}

/// 컴퓨터 이름을 기기 이름 후보로 쓴다 (1~64바이트).
fn default_device_name() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let mut name = String::new();
    for c in raw.trim().chars() {
        if name.len() + c.len_utf8() > 64 {
            break;
        }
        name.push(c);
    }
    if name.is_empty() {
        "desktop".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_name_is_never_empty_or_too_long() {
        let n = default_device_name();
        assert!(!n.is_empty() && n.len() <= 64);
    }
}
