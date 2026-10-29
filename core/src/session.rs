//! 릴레이 연결과 앱 상태를 이어 주는 세션: 받은 메시지를 처리하고, 보내고, 구독을 맞춘다.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::api::{now_ms, Clip, ClipFerry, Error, GroupHealth, Incoming};
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
    /// 키가 회전되어 새 에포크로 넘어갔다.
    fn on_epoch_changed(&self, _epoch: u64) {}
    /// 그룹과의 관계가 정해졌다. `Removed`/`Stale`이면 다시 페어링해야 한다.
    fn on_group_health(&self, _health: GroupHealth) {}
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
type Subscribed = Arc<Mutex<HashSet<[u8; 32]>>>;

/// 릴레이에 붙어 있는 동안 살아 있는 세션. 드롭하면 연결을 닫는다.
pub struct Session {
    app: Arc<ClipFerry>,
    client: Client,
    pending: Pending,
    subscribed: Subscribed,
    connected: watch::Receiver<bool>,
    online: watch::Receiver<usize>,
    task: JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 앱이 원하는 구독(현재 + 유예 중인 이전 에포크)에 연결의 구독을 맞춘다.
fn sync_subscriptions(app: &ClipFerry, client: &Client, subscribed: &Subscribed) {
    let want = app.subscriptions();
    let mut have = subscribed.lock().unwrap();
    for s in &want {
        if have.insert(s.channel_id.0) {
            client.subscribe(s.channel_id.0, s.device_tag.0);
        }
    }
    let wanted: HashSet<[u8; 32]> = want.iter().map(|s| s.channel_id.0).collect();
    for channel in have.clone() {
        if !wanted.contains(&channel) {
            client.unsubscribe(channel);
            have.remove(&channel);
        }
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
        if self.group_health(now_ms()) == GroupHealth::Removed {
            return Err(Error::Removed);
        }
        self.connection().ok_or(Error::NoGroup)?;
        self.prune_expired(now_ms());

        let (client, mut events) = Client::connect(relay_url, config);
        let subscribed: Subscribed = Arc::default();
        sync_subscriptions(self, &client, &subscribed);

        let pending: Pending = Arc::default();
        let (connected_tx, connected) = watch::channel(false);
        let (online_tx, online) = watch::channel(0usize);
        let app = self.clone();
        let task_pending = pending.clone();
        let task_client = client.clone();
        let task_subscribed = subscribed.clone();
        // 이미 따라잡기 응답을 보낸 (에포크, 기기 태그)
        let mut pushed: HashSet<(u64, Vec<u8>)> = HashSet::new();

        let task = tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                match event {
                    Event::Connected(_) => {
                        let _ = connected_tx.send(true);
                        sink.on_status(ConnectionStatus::Connected);
                        app.prune_expired(now_ms());
                        sync_subscriptions(&app, &task_client, &task_subscribed);
                        // 밀려 있었다면 최신 키를 받고, 같은 에포크라면 생존 신호가 된다.
                        if let Ok(query) = app.seal_epoch_query(now_ms()) {
                            task_client.publish(query.channel_id.0, query.bytes, vec![]);
                        }
                        let health = app.group_health(now_ms());
                        if health != GroupHealth::Active {
                            sink.on_group_health(health);
                        }
                    }
                    Event::Disconnected => {
                        let _ = connected_tx.send(false);
                        let _ = online_tx.send(0);
                        sink.on_status(ConnectionStatus::Disconnected);
                    }
                    Event::Reconnecting { .. } => sink.on_status(ConnectionStatus::Reconnecting),
                    Event::Deliver(d) => {
                        match app.handle_deliver(&d.payload, Source::Relay, now_ms()) {
                            Ok(Incoming::Clip(clip)) => sink.on_clip(clip),
                            Ok(Incoming::DevicesChanged) => sink.on_devices_changed(),
                            Ok(Incoming::EpochChanged(epoch)) => {
                                sync_subscriptions(&app, &task_client, &task_subscribed);
                                sink.on_epoch_changed(epoch);
                                sink.on_devices_changed();
                                sink.on_group_health(GroupHealth::Active);
                            }
                            Ok(Incoming::Removed) => {
                                sync_subscriptions(&app, &task_client, &task_subscribed);
                                sink.on_group_health(GroupHealth::Removed);
                            }
                            Ok(Incoming::Replies(replies)) => {
                                for m in replies {
                                    task_client.publish(m.channel_id.0, m.bytes, vec![]);
                                }
                            }
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
                        let subs = app.subscriptions();
                        let Some(sub) = subs
                            .iter()
                            .find(|s| s.channel_id.0.as_slice() == p.channel_id.as_slice())
                        else {
                            continue;
                        };
                        if subs.first().map(|s| s.epoch) == Some(sub.epoch) {
                            let _ = online_tx.send(p.online_tags.len());
                        } else {
                            // 이전 에포크 채널에 아직 붙어 있는 기기가 있으면 최신 키를 먼저 보내 준다.
                            let fresh: Vec<Vec<u8>> = p
                                .online_tags
                                .iter()
                                .filter(|t| pushed.insert((sub.epoch, (*t).clone())))
                                .cloned()
                                .collect();
                            for m in app.catch_up_replies(sub.epoch, &fresh, now_ms()) {
                                task_client.publish(m.channel_id.0, m.bytes, vec![]);
                            }
                        }
                    }
                }
            }
        });

        Ok(Session {
            app: self.clone(),
            client,
            pending,
            subscribed,
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

    /// 게시하고 릴레이의 `Ack`(전달된 수)를 기다린다.
    async fn publish(&self, channel: [u8; 32], bytes: Vec<u8>) -> Result<u32, SendError> {
        let (tx, rx) = oneshot::channel();
        // 잠근 채로 게시하고 등록해서, 답이 먼저 도착해도 이벤트 처리가 등록을 기다리게 한다.
        let id = {
            let mut pending = self.pending.lock().unwrap();
            let id = self.client.publish(channel, bytes, vec![]);
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

    /// 텍스트를 그룹의 다른 기기들에게 보낸다. 릴레이가 실제로 전달한 수를 돌려준다.
    pub async fn send_text(&self, text: &str) -> Result<u32, SendError> {
        let sealed = self.app.seal_text(text, now_ms())?;
        self.publish(sealed.channel_id.0, sealed.bytes).await
    }

    /// 이 기기의 이름을 바꾸고 그룹에 알린다.
    pub async fn rename(&self, new_name: &str) -> Result<u32, SendError> {
        let sealed = self.app.rename(new_name, now_ms())?;
        self.publish(sealed.channel_id.0, sealed.bytes).await
    }

    /// 기기를 제거한다(키 회전). `Rekey`를 이전 채널에 게시한 뒤 새 채널로 구독을 옮긴다.
    /// 전달된 수(이전 채널에 붙어 있던 다른 기기 수)를 돌려준다.
    pub async fn remove_device(&self, target: &str) -> Result<u32, SendError> {
        let messages = self.app.remove_device(target, now_ms())?;
        let mut delivered = 0;
        for m in messages {
            // 아직 이전 채널을 구독 중이므로 게시할 수 있다.
            delivered += self.publish(m.channel_id.0, m.bytes).await?;
        }
        sync_subscriptions(&self.app, &self.client, &self.subscribed);
        Ok(delivered)
    }
}
