//! 코어와 화면(트레이·창)을 이어 주는 얇은 층.
//! 암호·프로토콜 로직은 없다. 코어를 열고, 세션과 페어링을 시작하고, 상태를 화면에 알린다.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use clip_ferry_core::api::{Clip, ClipFerry, Config, Error, GroupHealth, ImageClip};
use clip_ferry_core::crypto::DeviceId;
use clip_ferry_core::link::{AccountLink, LinkRequestInfo};
use clip_ferry_core::pair_flow::{self, InviteFlow, InviteUpdate, JoinFlow, JoinUpdate};
use clip_ferry_core::session::{ConnectionStatus, EventSink, Session};
use clip_ferry_core::store::SecretStore;
use clip_ferry_core::transfer::MAX_IMAGE_BYTES;
use clip_ferry_core::transport::{TokenSource, TransportConfig};
use tauri::{AppHandle, Emitter};
use tauri_plugin_autostart::ManagerExt;

use crate::account::{self, Account, TokenError};
use crate::clipsync::{ClipSync, Content};
use crate::imgcodec;
use crate::settings::{self, Settings};
use crate::sysclip::SystemClipboard;
use crate::tray;
use crate::view::{self, AppView, Connection, LinkRequestView, PairKind, PairPhase, PairingView};

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
    /// 계정 연결(F37)로 진행 중
    via_account: bool,
}

struct Inner {
    relay_url: String,
    account_url: String,
    /// 릴레이가 접속 토큰을 거절했다 (다시 연결되면 지운다)
    auth_rejected: bool,
    /// 브라우저 로그인을 기다리는 중
    login_task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// 로그인 화면에 보여 줄 안내 (실패·만료)
    login_message: Option<String>,
    send_enabled: bool,
    receive_enabled: bool,
    /// 트레이의 일시정지. 저장하지 않는다(앱을 다시 켜면 풀린다).
    paused: bool,
    autostart: bool,
    /// 지금 그룹 채널에 붙어 있는 기기의 짧은 id.
    online: HashSet<String>,
    connection: Connection,
    error: Option<String>,
    session: Option<Arc<Session>>,
    pairing: Option<Pairing>,
    next_pairing_id: u64,
    /// 계정 채널 정보(F37). 세션을 시작할 때 계정 서버에서 받는다.
    account_link: Option<AccountLink>,
    /// 같은 계정의 새 기기가 보낸 참여 요청 (사용자가 연결·무시를 고르기 전)
    link_request: Option<LinkRequestInfo>,
}

pub struct Shell {
    app: Arc<ClipFerry>,
    account: Arc<Account>,
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
            if !shell.can_receive() {
                return;
            }
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
            if !shell.can_receive() {
                return;
            }
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
            {
                let mut inner = shell.inner.lock().unwrap();
                inner.connection = status.into();
                if status == ConnectionStatus::Connected {
                    inner.auth_rejected = false;
                }
            }
            shell.publish();
        }
    }

    fn on_auth_required(&self) {
        // 접속 토큰을 거절당했다. 다음 재연결 때 새 토큰을 받는다(갱신이 거절되면 로그아웃 상태가 된다).
        if let Some(shell) = self.0.upgrade() {
            shell.account.invalidate_access();
            shell.inner.lock().unwrap().auth_rejected = true;
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

    fn on_link_request(&self, request: LinkRequestInfo) {
        if let Some(shell) = self.0.upgrade() {
            shell.on_link_request(request);
        }
    }

    fn on_presence(&self, online: Vec<DeviceId>) {
        if let Some(shell) = self.0.upgrade() {
            shell.inner.lock().unwrap().online =
                online.iter().map(|d| view::short_id(&d.0)).collect();
            shell.publish();
        }
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
        account_secrets: Box<dyn SecretStore>,
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
            account_url,
            sync_sensitive,
            send_enabled,
            receive_enabled,
        } = settings::load(&data_dir);
        let autostart = handle.autolaunch().is_enabled().unwrap_or(false);
        Ok(Arc::new(Self {
            app,
            account: Arc::new(Account::new(account_secrets)),
            clip: ClipSync::new(SystemClipboard::new()),
            sync_sensitive,
            handle,
            data_dir,
            seq: AtomicU64::new(1),
            inner: Mutex::new(Inner {
                relay_url,
                account_url,
                auth_rejected: false,
                login_task: None,
                login_message: None,
                send_enabled,
                receive_enabled,
                paused: false,
                autostart,
                online: HashSet::new(),
                connection: Connection::Reconnecting,
                error: None,
                session: None,
                pairing: None,
                next_pairing_id: 1,
                account_link: None,
                link_request: None,
            }),
        }))
    }

    /// 앱이 켜질 때 한 번. 로그인하지 않았으면 로그인 화면, 그룹이 있으면 릴레이에 연결하고,
    /// 없으면 첫 실행 마법사를 띄운다. tokio 런타임 안에서 불러야 한다.
    pub fn begin(self: &Arc<Self>) {
        self.watch_clipboard();
        if !self.account.is_signed_in() {
            self.publish();
            return;
        }
        self.register_device();
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
            move || {
                on.upgrade()
                    .is_some_and(|s| s.has_session() && s.can_send())
            },
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

    /// 지금 복사한 내용을 보내도 되는가 (일시정지가 아니고, 보내기가 켜져 있다).
    fn can_send(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.send_enabled && !inner.paused
    }

    /// 지금 받은 내용을 클립보드에 써도 되는가.
    fn can_receive(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.receive_enabled && !inner.paused
    }

    /// 복사한 내용을 그룹에 보낸다. 다시 시도할 필요가 없으면(보냈거나, 일부러 안 보냈으면) `true`.
    async fn send_clip(&self, content: Content) -> bool {
        // 읽은 뒤 일시정지되었을 수 있다. 보내지 않기로 한 것이므로 다시 시도하지 않는다.
        if !self.can_send() {
            return true;
        }
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

    /// 릴레이 연결 설정: 연결할 때마다 계정의 접속 토큰을 싣는다(만료가 가까우면 갱신).
    fn transport_config(self: &Arc<Self>) -> TransportConfig {
        let weak = Arc::downgrade(self);
        TransportConfig {
            token: Some(TokenSource::new(move || {
                let weak = weak.clone();
                async move {
                    let shell = weak.upgrade()?;
                    match shell.account.token().await {
                        Ok(token) => Some(token),
                        Err(TokenError::SignedOut) => {
                            // 세션을 멈추는 일은 이 연결 작업 밖에서 한다.
                            tauri::async_runtime::spawn(async move {
                                shell.signed_out(Some(
                                    "로그인이 만료되었습니다. 다시 로그인하세요".into(),
                                ));
                            });
                            None
                        }
                        // 계정 서버에 닿지 않으면 토큰 없이 시도하고, 다음 재연결 때 다시 받는다.
                        Err(TokenError::Unavailable) => None,
                    }
                }
            })),
            ..TransportConfig::default()
        }
    }

    fn require_sign_in(&self) -> Result<(), String> {
        if self.account.is_signed_in() {
            Ok(())
        } else {
            Err("먼저 로그인하세요".into())
        }
    }

    /// 릴레이 세션을 (다시) 시작한다. 로그인하지 않았으면 연결하지 않는다.
    /// tokio 런타임 안에서 불러야 한다.
    pub fn start(self: &Arc<Self>) {
        let config = self.transport_config();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.session = None;
            inner.auth_rejected = false;
            if !self.account.is_signed_in() {
                inner.connection = Connection::Stopped;
                drop(inner);
                self.publish();
                return;
            }
            let sink = Arc::new(Sink(Arc::downgrade(self)));
            match self.app.start_session(&inner.relay_url, config, sink) {
                Ok(session) => {
                    let session = Arc::new(session);
                    session.set_account_link(inner.account_link.clone());
                    self.listen_account(&session);
                    inner.session = Some(session);
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

    /// 지금 값에 `f`를 적용해 설정 파일을 쓴다. 일시정지·자동 시작은 저장하지 않는다.
    fn save_settings(&self, f: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let inner = self.inner.lock().unwrap();
        let mut s = Settings {
            relay_url: inner.relay_url.clone(),
            account_url: inner.account_url.clone(),
            sync_sensitive: self.sync_sensitive,
            send_enabled: inner.send_enabled,
            receive_enabled: inner.receive_enabled,
        };
        f(&mut s);
        settings::save(&self.data_dir, &s).map_err(|_| "설정을 저장하지 못했습니다".to_string())
    }

    /// 보내기·받기를 각각 켜고 끈다.
    pub fn set_sync(&self, send: bool, receive: bool) -> Result<AppView, String> {
        self.save_settings(|s| {
            s.send_enabled = send;
            s.receive_enabled = receive;
        })?;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.send_enabled = send;
            inner.receive_enabled = receive;
        }
        self.publish();
        Ok(self.view())
    }

    /// 일시정지(`true`)하거나 재개(`false`)한다. 일시정지 중에는 보내지도 받지도 않는다.
    pub fn set_paused(&self, paused: bool) -> AppView {
        self.inner.lock().unwrap().paused = paused;
        self.publish();
        self.view()
    }

    pub fn toggle_pause(&self) {
        let paused = self.inner.lock().unwrap().paused;
        self.set_paused(!paused);
    }

    /// 로그인할 때 자동으로 시작할지 정한다.
    pub fn set_autostart(&self, on: bool) -> Result<AppView, String> {
        let launch = self.handle.autolaunch();
        let result = if on {
            launch.enable()
        } else {
            launch.disable()
        };
        result.map_err(|_| "자동 시작 설정을 바꾸지 못했습니다".to_string())?;
        self.inner.lock().unwrap().autostart = launch.is_enabled().unwrap_or(on);
        self.publish();
        Ok(self.view())
    }

    pub fn set_relay_url(self: &Arc<Self>, input: &str) -> Result<AppView, String> {
        let relay_url = settings::validate_relay_url(input)?;
        self.save_settings(|s| s.relay_url = relay_url.clone())?;
        self.inner.lock().unwrap().relay_url = relay_url;
        // 그룹이 아직 없으면 연결할 것이 없다.
        if self.app.connection().is_some() {
            self.start();
        } else {
            self.publish();
        }
        Ok(self.view())
    }

    // ------------------------------------------------------------ 계정 (F34)

    /// 로그인 전에만 바꿀 수 있다(로그인 정보는 그 계정 서버의 것이다).
    pub fn set_account_url(&self, input: &str) -> Result<AppView, String> {
        if self.account.is_signed_in() {
            return Err("로그아웃한 뒤에 바꿀 수 있습니다".into());
        }
        let url = settings::validate_account_url(input)?;
        self.save_settings(|s| s.account_url = url.clone())?;
        self.inner.lock().unwrap().account_url = url;
        self.publish();
        Ok(self.view())
    }

    /// 구글 로그인을 시작한다: 시스템 브라우저를 열고, 돌아오면 로그인을 마친다.
    pub fn start_google_login(self: &Arc<Self>) -> AppView {
        let url = {
            let mut inner = self.inner.lock().unwrap();
            if inner.login_task.is_some() || self.account.is_signed_in() {
                drop(inner);
                return self.view();
            }
            inner.login_message = None;
            inner.account_url.clone()
        };
        let weak = Arc::downgrade(self);
        let account = self.account.clone();
        let task = tauri::async_runtime::spawn(async move {
            let result = account.google_login(&url, account::open_browser).await;
            if let Some(shell) = weak.upgrade() {
                shell.login_finished(result);
            }
        });
        self.inner.lock().unwrap().login_task = Some(task);
        self.publish();
        self.view()
    }

    /// 개발용 로그인 (디버그 빌드, 계정 서버 `--dev-login`).
    #[cfg(debug_assertions)]
    pub async fn dev_login(self: &Arc<Self>, email: &str) -> Result<AppView, String> {
        let url = self.inner.lock().unwrap().account_url.clone();
        let result = self.account.dev_login(&url, email).await;
        let failed = result.as_ref().err().cloned();
        self.login_finished(result);
        match failed {
            Some(e) => Err(e),
            None => Ok(self.view()),
        }
    }

    /// 브라우저 로그인을 더 기다리지 않는다.
    pub fn cancel_login(&self) -> AppView {
        if let Some(task) = self.inner.lock().unwrap().login_task.take() {
            task.abort();
        }
        self.publish();
        self.view()
    }

    fn login_finished(self: &Arc<Self>, result: Result<account::Profile, String>) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.login_task = None;
            inner.login_message = result.as_ref().err().cloned();
        }
        if result.is_ok() {
            self.register_device();
            // 그룹이 있으면(기존 사용자) 바로 연결하고, 없으면 마법사로 간다.
            if self.app.connection().is_some() {
                self.start();
                return;
            }
        }
        self.publish();
    }

    /// 로그아웃: 연결과 페어링을 멈추고 이 기기의 로그인 정보를 지운다. 그룹·페어링은 그대로 둔다.
    pub async fn logout(self: &Arc<Self>) -> AppView {
        self.account.logout().await;
        self.signed_out(None);
        self.view()
    }

    /// 로그인하지 않은 상태로 돌아간다. 그룹은 그대로이고 다시 로그인하면 이어서 쓴다.
    fn signed_out(&self, message: Option<String>) {
        let (session, pairing) = {
            let mut inner = self.inner.lock().unwrap();
            inner.online.clear();
            inner.connection = Connection::Stopped;
            inner.auth_rejected = false;
            inner.login_message = message;
            inner.account_link = None;
            inner.link_request = None;
            (inner.session.take(), inner.pairing.take())
        };
        // 잠금을 놓은 뒤에 멈춘다.
        drop(session);
        drop(pairing);
        self.publish();
    }

    /// 이 기기를 계정의 기기 목록에 올린다(이름·플랫폼). 실패해도 동작에는 상관없다.
    fn register_device(&self) {
        let Ok(status) = self.app.status() else {
            return;
        };
        let id: String = status
            .device_id
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let name = status.device_name;
        let account = self.account.clone();
        tauri::async_runtime::spawn(async move {
            let _ = account.register_device(&id, &name).await;
        });
    }

    // ------------------------------------------------------------ 기기 관리

    fn session(&self) -> Result<Arc<Session>, String> {
        self.inner
            .lock()
            .unwrap()
            .session
            .clone()
            .ok_or_else(|| "릴레이에 연결된 뒤에 할 수 있습니다".to_string())
    }

    /// 이 기기의 이름을 바꾸고 그룹에 알린다.
    pub async fn rename_device(&self, name: &str) -> Result<AppView, String> {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("이름은 1~64바이트여야 합니다".into());
        }
        self.session()?
            .rename(name)
            .await
            .map_err(|e| e.to_string())?;
        self.register_device();
        self.publish();
        Ok(self.view())
    }

    /// 다른 기기를 그룹에서 제거한다(키 회전). `id`는 목록에 보이는 짧은 id.
    pub async fn remove_device(&self, id: &str) -> Result<AppView, String> {
        let me = self.app.status().map_err(|e| e.to_string())?.device_id;
        if view::short_id(&me.0) == id {
            return Err("이 기기는 제거할 수 없습니다".into());
        }
        self.session()?
            .remove_device(id)
            .await
            .map_err(|e| e.to_string())?;
        self.publish();
        Ok(self.view())
    }

    /// 제거되었거나 오래 연결되지 않은 기기가 그룹에서 나가 처음 화면으로 돌아간다(다시 페어링하려고).
    pub fn leave_group(&self) -> Result<AppView, String> {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.session = None;
            inner.pairing = None;
            inner.online.clear();
            inner.connection = Connection::Stopped;
            inner.error = None;
        }
        self.app.leave_group().map_err(|e| e.to_string())?;
        self.publish();
        Ok(self.view())
    }

    // ------------------------------------------------------------ 첫 실행

    /// 기기가 아직 없으면 이름을 정해 만든다. 이미 있으면 그대로 둔다. 로그인해야 한다.
    fn ensure_device(&self, name: &str) -> Result<(), String> {
        self.require_sign_in()?;
        if !self.app.is_initialized() {
            self.app
                .init_device(name.trim())
                .map_err(|e| e.to_string())?;
            self.register_device();
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
            via_account: false,
        });
        drop(old);

        let weak = Arc::downgrade(self);
        let flow = pair_flow::start_join(
            &self.app,
            code,
            None,
            &relay,
            self.transport_config(),
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
                let via_account = self
                    .inner
                    .lock()
                    .unwrap()
                    .pairing
                    .as_ref()
                    .is_some_and(|p| p.id == id && p.via_account);
                self.update_pairing(id, |v| {
                    v.message = Some(if via_account {
                        "같은 계정의 다른 기기에서 연결을 눌러 주기를 기다리는 중입니다. 그 기기에서 clip-ferry가 켜져 있어야 합니다".into()
                    } else {
                        "초대한 기기의 응답을 기다리는 중입니다".into()
                    });
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
        self.require_sign_in()?;
        if self.app.connection().is_none() {
            return Err("먼저 그룹이 있어야 합니다".into());
        }
        let id = self.new_pairing_id();
        let relay = self.inner.lock().unwrap().relay_url.clone();
        let weak = Arc::downgrade(self);
        let flow =
            pair_flow::start_invite(&self.app, &relay, self.transport_config(), move |update| {
                if let Some(shell) = weak.upgrade() {
                    shell.on_invite_update(id, update);
                }
            })
            .map_err(|e| e.to_string())?;

        let mut pv = PairingView::new(PairKind::Invite, PairPhase::Waiting);
        pv.text_code = Some(flow.text_code.clone());
        pv.qr = view::qr(&flow.qr_string);
        pv.expires_at_ms = Some(flow.expires_at_ms);
        let old = self.inner.lock().unwrap().pairing.replace(Pairing {
            id,
            view: pv,
            flow: Some(Flow::Invite(flow)),
            via_account: false,
        });
        drop(old);
        self.publish();
        Ok(self.view())
    }

    // ------------------------------------------------------------ 계정 연결 (F37)

    /// 세션이 계정 채널을 듣게 한다. 계정 서버에 닿지 않으면 잠시 뒤 다시 시도한다.
    fn listen_account(self: &Arc<Self>, session: &Arc<Session>) {
        if self.inner.lock().unwrap().account_link.is_some() {
            return;
        }
        let weak = Arc::downgrade(self);
        let target = Arc::downgrade(session);
        tauri::async_runtime::spawn(async move {
            loop {
                let Some(shell) = weak.upgrade() else { return };
                // 그 사이 세션이 바뀌었으면(다시 시작·로그아웃) 그만둔다.
                let Some(session) = target.upgrade() else {
                    return;
                };
                match shell.account.account_link().await {
                    Ok(link) => {
                        session.set_account_link(Some(link.clone()));
                        shell.inner.lock().unwrap().account_link = Some(link);
                        return;
                    }
                    Err(TokenError::SignedOut) => return,
                    Err(TokenError::Unavailable) => {}
                }
                drop((shell, session));
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
    }

    fn on_link_request(&self, request: LinkRequestInfo) {
        let name = request.name.clone();
        {
            let mut inner = self.inner.lock().unwrap();
            // 다른 페어링을 하는 중에는 받지 않는다(새 기기가 다시 보내면 그때 보인다).
            if inner.pairing.is_some() {
                return;
            }
            inner.link_request = Some(request);
        }
        self.notify(&format!(
            "새 기기 '{name}'이(가) 같은 계정으로 연결을 요청합니다. clip-ferry 창에서 확인하세요"
        ));
        self.publish();
    }

    /// 새 기기의 참여 요청에 응한다: 확인 숫자를 맞춰 보는 화면으로 간다.
    pub fn accept_link_request(self: &Arc<Self>) -> Result<AppView, String> {
        self.require_sign_in()?;
        let (request, link, relay) = {
            let mut inner = self.inner.lock().unwrap();
            let request = inner
                .link_request
                .take()
                .ok_or_else(|| "참여 요청이 없습니다".to_string())?;
            let link = inner
                .account_link
                .clone()
                .ok_or_else(|| "계정 정보를 아직 받지 못했습니다".to_string())?;
            (request, link, inner.relay_url.clone())
        };
        if clip_ferry_core::api::now_ms() >= request.expires_at_ms {
            self.publish();
            return Err("참여 요청이 만료되었습니다. 새 기기에서 다시 시도하세요".into());
        }
        let id = self.new_pairing_id();
        let weak = Arc::downgrade(self);
        let flow = pair_flow::start_link_invite(
            &self.app,
            &relay,
            self.transport_config(),
            link.channel(),
            &request,
            move |update| {
                if let Some(shell) = weak.upgrade() {
                    shell.on_invite_update(id, update);
                }
            },
        )
        .map_err(|e| e.to_string())?;
        let mut pv = PairingView::new(PairKind::Invite, PairPhase::Waiting);
        pv.peer_name = Some(request.name);
        pv.expires_at_ms = Some(flow.expires_at_ms);
        pv.message = Some("새 기기의 응답을 기다리는 중입니다".into());
        let old = self.inner.lock().unwrap().pairing.replace(Pairing {
            id,
            view: pv,
            flow: Some(Flow::Invite(flow)),
            via_account: true,
        });
        drop(old);
        self.publish();
        Ok(self.view())
    }

    pub fn dismiss_link_request(&self) -> AppView {
        self.inner.lock().unwrap().link_request = None;
        self.publish();
        self.view()
    }

    /// 같은 계정으로 로그인한 다른 기기에 참여를 요청한다(QR·코드 없이).
    pub async fn start_link_join(self: &Arc<Self>, name: &str) -> Result<AppView, String> {
        self.ensure_device(name)?;
        let (sign_pub, dh_pub, device_name) =
            self.app.link_identity().map_err(|e| e.to_string())?;
        let ticket = self
            .account
            .link_ticket(&sign_pub, &dh_pub, &device_name)
            .await?;
        let id = self.new_pairing_id();
        let relay = self.inner.lock().unwrap().relay_url.clone();
        let old = self.inner.lock().unwrap().pairing.replace(Pairing {
            id,
            view: PairingView::new(PairKind::Join, PairPhase::Connecting),
            flow: None,
            via_account: true,
        });
        drop(old);

        let weak = Arc::downgrade(self);
        let flow = pair_flow::start_link_join(
            &self.app,
            &relay,
            self.transport_config(),
            ticket,
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
        let (connection, relay_url, error, pairing, online, flags, login, link_request) = {
            let inner = self.inner.lock().unwrap();
            let error = inner.error.clone().or_else(|| {
                inner.auth_rejected.then(|| {
                    "릴레이가 로그인(접속 토큰)을 받지 않았습니다. 다시 연결하는 중입니다"
                        .to_string()
                })
            });
            (
                inner.connection,
                inner.relay_url.clone(),
                error,
                inner.pairing.as_ref().map(|p| p.view.clone()),
                inner.online.clone(),
                (
                    inner.send_enabled,
                    inner.receive_enabled,
                    inner.paused,
                    inner.autostart,
                ),
                (
                    inner.account_url.clone(),
                    inner.login_task.is_some(),
                    inner.login_message.clone(),
                ),
                inner
                    .link_request
                    .as_ref()
                    .filter(|r| r.expires_at_ms > clip_ferry_core::api::now_ms())
                    .map(|r| LinkRequestView {
                        name: r.name.clone(),
                        expires_at_ms: r.expires_at_ms,
                    }),
            )
        };
        let mut view = view::build(&self.app, connection, &relay_url, error);
        view.pairing = pairing;
        view.link_request = link_request;
        (
            view.send_enabled,
            view.receive_enabled,
            view.paused,
            view.autostart,
        ) = flags;
        for d in &mut view.devices {
            d.online = online.contains(&d.id) || (d.is_self && connection == Connection::Connected);
        }
        view.default_name = default_device_name();
        view.account = self.account.profile().map(|p| view::AccountView {
            email: p.email,
            name: p.name,
        });
        (view.account_url, view.login_busy, view.login_message) = login;
        view.dev_login = cfg!(debug_assertions);
        if view.account.is_none() {
            // 로그인 전에는 연결하지 않는다.
            view.connection = Connection::Stopped;
        }
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
