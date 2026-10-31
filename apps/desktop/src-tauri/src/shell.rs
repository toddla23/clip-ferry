//! 코어와 화면(트레이·창)을 이어 주는 얇은 층.
//! 암호·프로토콜 로직은 없다. 코어를 열고, 세션을 시작하고, 상태를 화면에 알린다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use clip_ferry_core::api::{Clip, ClipFerry, Config, Error, GroupHealth};
use clip_ferry_core::session::{ConnectionStatus, EventSink, Session};
use clip_ferry_core::store::SecretStore;
use clip_ferry_core::transport::TransportConfig;
use tauri::{AppHandle, Emitter};

use crate::settings::{self, Settings};
use crate::tray;
use crate::view::{self, AppView, Connection};

struct Inner {
    relay_url: String,
    connection: Connection,
    error: Option<String>,
    session: Option<Session>,
}

pub struct Shell {
    app: Arc<ClipFerry>,
    handle: AppHandle,
    data_dir: PathBuf,
    seq: AtomicU64,
    inner: Mutex<Inner>,
}

/// 코어 이벤트를 받아 화면을 갱신한다. 셸이 사라져도 안전하도록 약한 참조만 쥔다.
struct Sink(Weak<Shell>);

impl EventSink for Sink {
    fn on_clip(&self, _clip: Clip) {
        // 클립보드 연동은 F11. 받은 내용은 여기서 쓰지도 기록하지도 않는다.
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
        let Settings { relay_url } = settings::load(&data_dir);
        Ok(Arc::new(Self {
            app,
            handle,
            data_dir,
            seq: AtomicU64::new(1),
            inner: Mutex::new(Inner {
                relay_url,
                connection: Connection::Reconnecting,
                error: None,
                session: None,
            }),
        }))
    }

    /// 처음 실행이면 기기와 그룹을 만든다. (페어링 마법사는 F10)
    pub fn ensure_group(&self) -> Result<(), Error> {
        if !self.app.is_initialized() {
            self.app.init_device(&default_device_name())?;
        }
        if self.app.status()?.epoch.is_none() {
            self.app.create_group()?;
        }
        Ok(())
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

    /// 시작 단계의 오류를 화면에 남긴다.
    pub fn fail(&self, error: &Error) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.connection = Connection::Stopped;
            inner.error = Some(error.to_string());
        }
        self.publish();
    }

    pub fn set_relay_url(self: &Arc<Self>, input: &str) -> Result<AppView, String> {
        let relay_url = settings::validate_relay_url(input)?;
        settings::save(
            &self.data_dir,
            &Settings {
                relay_url: relay_url.clone(),
            },
        )
        .map_err(|_| "설정을 저장하지 못했습니다".to_string())?;
        self.inner.lock().unwrap().relay_url = relay_url;
        self.start();
        Ok(self.view())
    }

    pub fn view(&self) -> AppView {
        let (connection, relay_url, error) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.connection,
                inner.relay_url.clone(),
                inner.error.clone(),
            )
        };
        let mut view = view::build(&self.app, connection, &relay_url, error);
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

/// 컴퓨터 이름을 기기 이름으로 쓴다 (1~64바이트).
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
