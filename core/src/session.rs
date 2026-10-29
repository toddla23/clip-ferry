//! 릴레이 연결과 앱 상태를 이어 주는 세션: 받은 메시지를 처리하고, 텍스트를 보낸다.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::api::{now_ms, Clip, ClipFerry, Error, Incoming};
use crate::envelope::Source;
use crate::pb::ErrorCode;
use crate::transport::{Client, Event, TransportConfig};

/// 셸이 구현하는 알림 창구.
pub trait EventSink: Send + Sync {
    /// 다른 기기가 보낸 텍스트를 받았다.
    fn on_clip(&self, clip: Clip);
    fn on_status(&self, _status: ConnectionStatus) {}
    /// 기기 목록(로스터)이 바뀌었다.
    fn on_devices_changed(&self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    /// 연결이 끊겼다.
    Disconnected,
    /// 다시 연결을 시도하는 중이다.
    Reconnecting,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error(transparent)]
    App(#[from] Error),
    #[error("릴레이에 연결되어 있지 않습니다")]
    NotConnected,
    #[error("릴레이가 거부했습니다: {0:?}")]
    Relay(ErrorCode),
    #[error("릴레이 응답을 기다리다 시간이 초과되었습니다")]
    Timeout,
}

type Pending = Arc<Mutex<HashMap<u32, oneshot::Sender<Result<u32, SendError>>>>>;

/// 릴레이에 붙어 있는 동안 살아 있는 세션. 드롭하면 연결을 닫는다.
pub struct Session {
    app: Arc<ClipFerry>,
    client: Client,
    pending: Pending,
    connected: watch::Receiver<bool>,
    online: watch::Receiver<usize>,
    task: JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ClipFerry {
    /// 그룹 채널을 구독하고 받은 메시지를 `sink`로 전달하는 세션을 시작한다.
    /// tokio 런타임 안에서 불러야 한다.
    pub fn start_session(
        self: &Arc<Self>,
        relay_url: &str,
        config: TransportConfig,
        sink: Arc<dyn EventSink>,
    ) -> Result<Session, Error> {
        let conn = self.connection().ok_or(Error::NoGroup)?;
        let (client, mut events) = Client::connect(relay_url, config);
        client.subscribe(conn.channel_id.0, conn.device_tag.0);

        let pending: Pending = Arc::default();
        let (connected_tx, connected) = watch::channel(false);
        let (online_tx, online) = watch::channel(0usize);
        let app = self.clone();
        let task_pending = pending.clone();

        let task = tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                match event {
                    Event::Connected(_) => {
                        let _ = connected_tx.send(true);
                        sink.on_status(ConnectionStatus::Connected);
                    }
                    Event::Disconnected => {
                        let _ = connected_tx.send(false);
                        let _ = online_tx.send(0);
                        sink.on_status(ConnectionStatus::Disconnected);
                    }
                    Event::Reconnecting { .. } => sink.on_status(ConnectionStatus::Reconnecting),
                    Event::Deliver(d) => {
                        // 지금 그룹 채널로 온 것만 처리한다.
                        let current = app.connection().map(|c| c.channel_id.0);
                        if current.as_ref().map(|c| c.as_slice()) != Some(d.channel_id.as_slice()) {
                            continue;
                        }
                        match app.handle_deliver(&d.payload, Source::Relay, now_ms()) {
                            Ok(Incoming::Clip(clip)) => sink.on_clip(clip),
                            Ok(Incoming::DevicesChanged) => sink.on_devices_changed(),
                            Ok(Incoming::Nothing) | Err(_) => {}
                        }
                    }
                    Event::Ack(a) => resolve(&task_pending, a.request_id, Ok(a.delivered)),
                    Event::Error(e) => {
                        resolve(&task_pending, e.request_id, Err(SendError::Relay(e.code())))
                    }
                    Event::PublishFailed { request_id } => {
                        resolve(&task_pending, request_id, Err(SendError::NotConnected))
                    }
                    Event::Presence(p) => {
                        let current = app.connection().map(|c| c.channel_id.0);
                        if current.as_ref().map(|c| c.as_slice()) == Some(p.channel_id.as_slice()) {
                            let _ = online_tx.send(p.online_tags.len());
                        }
                    }
                }
            }
        });

        Ok(Session {
            app: self.clone(),
            client,
            pending,
            connected,
            online,
            task,
        })
    }
}

fn resolve(pending: &Pending, request_id: u32, result: Result<u32, SendError>) {
    if let Some(tx) = pending.lock().unwrap().remove(&request_id) {
        let _ = tx.send(result);
    }
}

impl Session {
    /// 릴레이에 연결될 때까지 기다린다. 시간 안에 연결되면 `true`.
    pub async fn wait_connected(&self, wait: Duration) -> bool {
        let mut rx = self.connected.clone();
        timeout(wait, rx.wait_for(|up| *up))
            .await
            .is_ok_and(|r| r.is_ok())
    }

    /// 그룹 채널에 이 기기를 포함해 `count`대 이상이 붙을 때까지 기다린다.
    pub async fn wait_online(&self, count: usize, wait: Duration) -> bool {
        let mut rx = self.online.clone();
        timeout(wait, rx.wait_for(|n| *n >= count))
            .await
            .is_ok_and(|r| r.is_ok())
    }

    /// 텍스트를 그룹의 다른 기기들에게 보낸다. 릴레이가 실제로 전달한 수를 돌려준다.
    pub async fn send_text(&self, text: &str) -> Result<u32, SendError> {
        let sealed = self.app.seal_text(text, now_ms())?;
        let (tx, rx) = oneshot::channel();
        // 잠근 채로 게시하고 등록해서, 답이 먼저 도착해도 이벤트 처리가 등록을 기다리게 한다.
        let id = {
            let mut pending = self.pending.lock().unwrap();
            let id = self
                .client
                .publish(sealed.channel_id.0, sealed.bytes, vec![]);
            pending.insert(id, tx);
            id
        };
        match timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(SendError::NotConnected),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(SendError::Timeout)
            }
        }
    }
}
