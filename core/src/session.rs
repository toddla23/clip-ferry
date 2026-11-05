//! 릴레이 연결과 앱 상태를 이어 주는 세션: 받은 메시지를 처리하고, 보내고, 구독을 맞춘다.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::api::{now_ms, Clip, ClipFerry, Error, GroupHealth, ImageClip, Incoming};
use crate::crypto::DeviceId;
use crate::envelope::Source;
use crate::pb::{ErrorCode, Platform, PushTarget};
use crate::transport::{Client, Event, TransportConfig};

/// 셸이 구현하는 알림 창구.
pub trait EventSink: Send + Sync {
    /// 다른 기기가 보낸 텍스트를 받았다.
    fn on_clip(&self, clip: Clip);
    /// 다른 기기가 보낸 이미지가 다 도착했다 (크기·해시 확인 끝, 디코딩은 셸이 한다).
    fn on_image(&self, _image: ImageClip) {}
    fn on_status(&self, _status: ConnectionStatus) {}
    /// 기기 목록(로스터)이 바뀌었다.
    fn on_devices_changed(&self) {}
    /// 키가 회전되어 새 에포크로 넘어갔다.
    fn on_epoch_changed(&self, _epoch: u64) {}
    /// 그룹과의 관계가 정해졌다. `Removed`/`Stale`이면 다시 페어링해야 한다.
    fn on_group_health(&self, _health: GroupHealth) {}
    /// 그룹 채널에 붙어 있는 기기가 바뀌었다 (이 기기 포함). 연결이 끊기면 빈 목록이 온다.
    fn on_presence(&self, _online: Vec<DeviceId>) {}
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
type LastSentSlot = Arc<Mutex<Option<LastSent>>>;

/// 보낸 클립을 들고 있는 시간. 이 안에 재전송 요청이 오면 다시 보낸다 (03-protocol 6절).
pub const RESEND_KEEP_MS: u64 = 10 * 60 * 1000;

/// 마지막으로 보낸 클립 (메모리에만).
struct LastSent {
    /// 받는 쪽이 알 수 있는 id: 텍스트 메시지 또는 `ImageStart`
    msg_id: [u8; 16],
    at_ms: u64,
    content: LastContent,
}

#[derive(Clone)]
enum LastContent {
    Text(String),
    Image {
        bytes: Vec<u8>,
        mime: String,
        width: u32,
        height: u32,
    },
}

/// 릴레이에 붙어 있는 동안 살아 있는 세션. 드롭하면 연결을 닫는다.
pub struct Session {
    app: Arc<ClipFerry>,
    client: Client,
    pending: Pending,
    subscribed: Subscribed,
    connected: watch::Receiver<bool>,
    online: watch::Receiver<usize>,
    last_sent: LastSentSlot,
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
        let last_sent: LastSentSlot = Arc::default();
        let task_last_sent = last_sent.clone();
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
                        sink.on_presence(Vec::new());
                        sink.on_status(ConnectionStatus::Disconnected);
                    }
                    Event::Reconnecting { .. } => sink.on_status(ConnectionStatus::Reconnecting),
                    Event::Deliver(d) => {
                        match app.handle_deliver(&d.payload, Source::Relay, now_ms()) {
                            Ok(Incoming::Clip(clip)) => sink.on_clip(clip),
                            Ok(Incoming::Image(image)) => sink.on_image(image),
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
                            Ok(Incoming::ResendRequested { from, msg_id }) => {
                                if let Some(content) = take_for_resend(&task_last_sent, &msg_id) {
                                    tokio::spawn(resend(
                                        app.clone(),
                                        task_client.clone(),
                                        task_pending.clone(),
                                        from,
                                        content,
                                    ));
                                }
                            }
                            // 푸시로만 온다. 세션에서는 할 일이 없다.
                            Ok(Incoming::ImageAnnounced { .. }) => {}
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
                            sink.on_presence(app.online_devices(&p.online_tags));
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
            last_sent,
            task,
        })
    }
}

/// 요청이 마지막으로 보낸 것을 가리키고 아직 들고 있으면 다시 보낼 내용을 꺼낸다.
/// `msg_id`가 비어 있으면 마지막 것.
fn take_for_resend(slot: &LastSentSlot, msg_id: &[u8]) -> Option<LastContent> {
    let last = slot.lock().unwrap();
    let last = last.as_ref()?;
    if now_ms().saturating_sub(last.at_ms) > RESEND_KEEP_MS {
        return None;
    }
    if !msg_id.is_empty() && msg_id != last.msg_id {
        return None;
    }
    Some(last.content.clone())
}

/// 요청한 기기에게만 다시 봉인해 보낸다. 실패해도 조용히 끝낸다(요청한 쪽이 기다리다 안내한다).
async fn resend(
    app: Arc<ClipFerry>,
    client: Client,
    pending: Pending,
    to: DeviceId,
    content: LastContent,
) {
    let Some(tag) = app.device_tag_of(&to) else {
        return;
    };
    let messages = match content {
        LastContent::Text(t) => app.seal_text(&t, now_ms()).map(|m| vec![m]),
        LastContent::Image {
            bytes,
            mime,
            width,
            height,
        } => app.seal_image(&bytes, &mime, width, height, now_ms()),
    };
    let Ok(messages) = messages else {
        return;
    };
    for m in messages {
        let sent = publish_paced(
            &client,
            &pending,
            m.channel_id.0,
            m.bytes,
            vec![tag.0],
            vec![],
        );
        if sent.await.is_err() {
            return;
        }
    }
}

/// 게시하고 릴레이의 `Ack`(전달된 수)를 기다린다.
async fn publish_once(
    client: &Client,
    pending: &Pending,
    channel: [u8; 32],
    bytes: Vec<u8>,
    to: Vec<[u8; 16]>,
    push: Vec<PushTarget>,
) -> Result<u32, SendError> {
    let (tx, rx) = oneshot::channel();
    // 잠근 채로 게시하고 등록해서, 답이 먼저 도착해도 이벤트 처리가 등록을 기다리게 한다.
    let id = {
        let mut pending = pending.lock().unwrap();
        let id = client.publish_with_push(channel, bytes, to, push);
        pending.insert(id, tx);
        id
    };
    match timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(SendError::NotConnected),
        Err(_) => {
            pending.lock().unwrap().remove(&id);
            Err(SendError::Timeout)
        }
    }
}

/// [`publish_once`]와 같되, 릴레이가 `RateLimited`로 거부하면 50 ms 쉬고 다시 게시한다(최대 100번).
async fn publish_paced(
    client: &Client,
    pending: &Pending,
    channel: [u8; 32],
    bytes: Vec<u8>,
    to: Vec<[u8; 16]>,
    push: Vec<PushTarget>,
) -> Result<u32, SendError> {
    let mut tries = 0;
    loop {
        let result = publish_once(
            client,
            pending,
            channel,
            bytes.clone(),
            to.clone(),
            push.clone(),
        )
        .await;
        match result {
            Err(SendError::Relay(ErrorCode::RateLimited)) if tries < 100 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            other => return other,
        }
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
        publish_once(&self.client, &self.pending, channel, bytes, vec![], vec![]).await
    }

    fn remember(&self, msg_id: [u8; 16], content: LastContent) {
        *self.last_sent.lock().unwrap() = Some(LastSent {
            msg_id,
            at_ms: now_ms(),
            content,
        });
    }

    /// 텍스트를 그룹의 다른 기기들에게 보낸다. 릴레이가 실제로 전달한 수를 돌려준다.
    /// 푸시 토큰이 있는 기기가 오프라인이면 릴레이가 푸시로 알린다. 보낸 것은 10분 들고 있다(재전송용).
    pub async fn send_text(&self, text: &str) -> Result<u32, SendError> {
        let sealed = self.app.seal_text(text, now_ms())?;
        self.remember(sealed.msg_id, LastContent::Text(text.to_string()));
        let push = self.app.push_targets();
        publish_once(
            &self.client,
            &self.pending,
            sealed.channel_id.0,
            sealed.bytes,
            vec![],
            push,
        )
        .await
    }

    /// 이미지를 그룹의 다른 기기들에게 보낸다 (PNG·JPEG·GIF·WebP, 최대 20 MiB).
    /// 청크를 하나씩 순서대로 게시한다. 릴레이의 속도 제한에 걸리면 잠깐 쉬고 다시 보낸다.
    /// 마지막 청크를 릴레이가 전달한 수를 돌려준다.
    pub async fn send_image(
        &self,
        bytes: &[u8],
        mime: &str,
        width: u32,
        height: u32,
    ) -> Result<u32, SendError> {
        let messages = self.app.seal_image(bytes, mime, width, height, now_ms())?;
        if let Some(start) = messages.first() {
            self.remember(
                start.msg_id,
                LastContent::Image {
                    bytes: bytes.to_vec(),
                    mime: mime.to_string(),
                    width,
                    height,
                },
            );
        }
        let mut delivered = 0;
        for (i, m) in messages.into_iter().enumerate() {
            // 푸시는 시작에만 붙인다. 받는 쪽은 "이미지 도착"만 알리고 열 때 다시 받아 간다.
            let push = if i == 0 {
                self.app.push_targets()
            } else {
                vec![]
            };
            delivered = publish_paced(
                &self.client,
                &self.pending,
                m.channel_id.0,
                m.bytes,
                vec![],
                push,
            )
            .await?;
        }
        Ok(delivered)
    }

    /// 이 기기의 푸시 토큰을 로스터에 적고 그룹에 알린다. 바뀐 것이 없으면 게시하지 않고 0.
    pub async fn set_push_token(&self, platform: Platform, token: &str) -> Result<u32, SendError> {
        match self.app.set_push_token(platform, token, now_ms())? {
            Some(sealed) => self.publish(sealed.channel_id.0, sealed.bytes).await,
            None => Ok(0),
        }
    }

    /// 보낸 기기(`from`)에게만 재전송을 요청한다. `msg_id`가 비어 있으면 마지막 것.
    /// 전달된 수를 돌려준다. 0이면 그 기기가 지금 오프라인이다.
    pub async fn request_resend(&self, from: &DeviceId, msg_id: &[u8]) -> Result<u32, SendError> {
        let tag = self
            .app
            .device_tag_of(from)
            .ok_or(SendError::App(Error::UnknownDevice))?;
        let sealed = self.app.seal_resend_request(msg_id, now_ms())?;
        publish_once(
            &self.client,
            &self.pending,
            sealed.channel_id.0,
            sealed.bytes,
            vec![tag.0],
            vec![],
        )
        .await
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
