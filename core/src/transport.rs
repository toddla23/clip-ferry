//! 릴레이 WebSocket 연결: 자동 재연결(지수 백오프), 재구독, 핑 (docs/03-protocol.md 5절).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::sync::mpsc;
use tokio::time::{interval_at, sleep, timeout, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::pb::{self, client_frame, relay_frame};

/// WebSocket 서브프로토콜 이름.
pub const SUBPROTOCOL: &str = "clip-ferry.v1";

#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub ping_interval: Duration,
    /// 이 시간 동안 서버에서 아무 프레임도 오지 않으면 연결이 죽은 것으로 본다.
    pub idle_timeout: Duration,
    pub connect_timeout: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(25),
            idle_timeout: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(10),
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
        }
    }
}

/// 연결 상태와 서버가 보낸 프레임.
#[derive(Debug)]
pub enum Event {
    /// 서버 `ServerHello`를 받음. 이 시점에 저장된 구독은 이미 다시 보낸 상태.
    Connected(pb::ServerHello),
    Disconnected,
    Reconnecting {
        attempt: u32,
        delay: Duration,
    },
    Deliver(pb::Deliver),
    Ack(pb::Ack),
    Error(pb::Error),
    Presence(pb::Presence),
    /// 연결이 끊긴 동안 보내려던 `Publish`는 큐에 쌓지 않고 실패로 알린다.
    PublishFailed {
        request_id: u32,
    },
}

enum Command {
    Subscribe { channel: [u8; 32], tag: [u8; 16] },
    Unsubscribe { channel: [u8; 32] },
    Publish(pb::Publish),
}

/// 릴레이 연결 핸들. 모든 `Client`가 드롭되면 백그라운드 작업이 끝난다.
#[derive(Clone)]
pub struct Client {
    cmd_tx: mpsc::UnboundedSender<Command>,
    next_request_id: Arc<AtomicU32>,
}

impl Client {
    /// `url`(예: `ws://127.0.0.1:8787`)에 연결을 시작한다. 연결 실패는 이벤트로 알리고 계속 재시도한다.
    pub fn connect(
        url: impl Into<String>,
        config: TransportConfig,
    ) -> (Client, mpsc::UnboundedReceiver<Event>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(run(url.into(), config, cmd_rx, ev_tx));
        (
            Client {
                cmd_tx,
                next_request_id: Arc::new(AtomicU32::new(1)),
            },
            ev_rx,
        )
    }

    /// 구독한다. 재연결하면 자동으로 다시 구독한다.
    pub fn subscribe(&self, channel: [u8; 32], tag: [u8; 16]) {
        let _ = self.cmd_tx.send(Command::Subscribe { channel, tag });
    }

    pub fn unsubscribe(&self, channel: [u8; 32]) {
        let _ = self.cmd_tx.send(Command::Unsubscribe { channel });
    }

    /// 게시하고 `request_id`를 돌려준다. 결과는 `Event::Ack` / `Event::Error` / `Event::PublishFailed`.
    pub fn publish(&self, channel: [u8; 32], payload: Vec<u8>, to: Vec<[u8; 16]>) -> u32 {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.cmd_tx.send(Command::Publish(pb::Publish {
            request_id,
            channel_id: channel.to_vec(),
            payload,
            to: to.into_iter().map(|t| t.to_vec()).collect(),
            push: Vec::new(),
        }));
        request_id
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

enum SessionEnd {
    /// 연결이 끊김. 재연결한다.
    Lost,
    /// 모든 `Client`가 사라짐. 종료한다.
    Closed,
}

fn frame_bytes(kind: client_frame::Kind) -> Message {
    Message::Binary(pb::ClientFrame { kind: Some(kind) }.encode_to_vec())
}

async fn connect(url: &str, config: &TransportConfig) -> Option<Ws> {
    let mut request = url.into_client_request().ok()?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(SUBPROTOCOL),
    );
    let (ws, _) = timeout(
        config.connect_timeout,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .ok()?
    .ok()?;
    Some(ws)
}

async fn run(
    url: String,
    config: TransportConfig,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    ev_tx: mpsc::UnboundedSender<Event>,
) {
    let mut subs: HashMap<[u8; 32], [u8; 16]> = HashMap::new();
    let mut attempt = 0u32;
    let mut delay = config.backoff_initial;

    loop {
        if let Some(ws) = connect(&url, &config).await {
            attempt = 0;
            delay = config.backoff_initial;
            let end = session(ws, &config, &mut subs, &mut cmd_rx, &ev_tx).await;
            let _ = ev_tx.send(Event::Disconnected);
            if matches!(end, SessionEnd::Closed) {
                return;
            }
        }

        attempt += 1;
        let _ = ev_tx.send(Event::Reconnecting { attempt, delay });
        // 기다리는 동안에도 명령은 받는다: 구독은 기억하고, 게시는 실패로 알린다.
        let wait = sleep(delay);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                _ = &mut wait => break,
                cmd = cmd_rx.recv() => match cmd {
                    None => return,
                    Some(Command::Subscribe { channel, tag }) => { subs.insert(channel, tag); }
                    Some(Command::Unsubscribe { channel }) => { subs.remove(&channel); }
                    Some(Command::Publish(p)) => {
                        let _ = ev_tx.send(Event::PublishFailed { request_id: p.request_id });
                    }
                },
            }
        }
        delay = (delay * 2).min(config.backoff_max);
    }
}

async fn session(
    ws: Ws,
    config: &TransportConfig,
    subs: &mut HashMap<[u8; 32], [u8; 16]>,
    cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
    ev_tx: &mpsc::UnboundedSender<Event>,
) -> SessionEnd {
    let (mut sink, mut stream) = ws.split();

    // 재연결 시 재구독
    for (channel, tag) in subs.iter() {
        let msg = frame_bytes(client_frame::Kind::Subscribe(pb::Subscribe {
            channel_id: channel.to_vec(),
            device_tag: tag.to_vec(),
        }));
        if sink.send(msg).await.is_err() {
            return SessionEnd::Lost;
        }
    }

    let mut ping = interval_at(Instant::now() + config.ping_interval, config.ping_interval);
    let mut last_rx = Instant::now();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let msg = match cmd {
                    None => return SessionEnd::Closed,
                    Some(Command::Subscribe { channel, tag }) => {
                        subs.insert(channel, tag);
                        frame_bytes(client_frame::Kind::Subscribe(pb::Subscribe {
                            channel_id: channel.to_vec(),
                            device_tag: tag.to_vec(),
                        }))
                    }
                    Some(Command::Unsubscribe { channel }) => {
                        subs.remove(&channel);
                        frame_bytes(client_frame::Kind::Unsubscribe(pb::Unsubscribe {
                            channel_id: channel.to_vec(),
                        }))
                    }
                    Some(Command::Publish(p)) => {
                        let request_id = p.request_id;
                        if sink.send(frame_bytes(client_frame::Kind::Publish(p))).await.is_err() {
                            let _ = ev_tx.send(Event::PublishFailed { request_id });
                            return SessionEnd::Lost;
                        }
                        continue;
                    }
                };
                if sink.send(msg).await.is_err() {
                    return SessionEnd::Lost;
                }
            }
            msg = stream.next() => {
                last_rx = Instant::now();
                match msg {
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Ok(frame) = pb::RelayFrame::decode(bytes.as_slice()) {
                            let event = match frame.kind {
                                Some(relay_frame::Kind::Hello(h)) => Some(Event::Connected(h)),
                                Some(relay_frame::Kind::Deliver(d)) => Some(Event::Deliver(d)),
                                Some(relay_frame::Kind::Ack(a)) => Some(Event::Ack(a)),
                                Some(relay_frame::Kind::Error(e)) => Some(Event::Error(e)),
                                Some(relay_frame::Kind::Presence(p)) => Some(Event::Presence(p)),
                                Some(relay_frame::Kind::Pong(_)) | None => None,
                            };
                            if let Some(event) = event {
                                let _ = ev_tx.send(event);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return SessionEnd::Lost,
                    Some(Ok(_)) => {}
                }
            }
            _ = ping.tick() => {
                if last_rx.elapsed() > config.idle_timeout {
                    return SessionEnd::Lost;
                }
                let msg = frame_bytes(client_frame::Kind::Ping(pb::Ping {}));
                if sink.send(msg).await.is_err() {
                    return SessionEnd::Lost;
                }
            }
        }
    }
}
