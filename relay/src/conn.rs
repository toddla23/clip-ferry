use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use clip_ferry_core::pb::{self, client_frame, relay_frame, ErrorCode};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::hub::{IpGuard, Outbox, Shared, Subscriber};
use crate::limits::Bucket;
use crate::SUBPROTOCOL;

/// 연결당 보낼 프레임 대기열. 가득 차면(느린 수신자) 그 수신자에게는 전달하지 않는다.
const OUTBOX_DEPTH: usize = 256;

fn encode(kind: relay_frame::Kind) -> Vec<u8> {
    pb::RelayFrame { kind: Some(kind) }.encode_to_vec()
}

struct Conn {
    id: u64,
    shared: Arc<Shared>,
    tx: Outbox,
    channels: HashSet<[u8; 32]>,
    frames: Bucket,
    bytes: Bucket,
}

pub async fn run_connection(mut socket: WebSocket, shared: Arc<Shared>, _guard: IpGuard) {
    shared.metrics.connection_opened();

    let negotiated = socket
        .protocol()
        .is_some_and(|p| p.as_bytes() == SUBPROTOCOL.as_bytes());
    if !negotiated {
        shared.metrics.error(ErrorCode::UnsupportedVersion);
        let frame = encode(relay_frame::Kind::Error(pb::Error {
            request_id: 0,
            code: ErrorCode::UnsupportedVersion as i32,
        }));
        let _ = socket.send(Message::Binary(frame)).await;
        shared.metrics.connection_closed();
        return;
    }

    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(OUTBOX_DEPTH);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if sink.send(Message::Binary(bytes)).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });

    let cfg = &shared.config;
    let mut conn = Conn {
        id: shared.next_conn_id(),
        tx,
        channels: HashSet::new(),
        frames: Bucket::new(
            f64::from(cfg.max_frames_per_sec),
            f64::from(cfg.max_frames_per_sec),
        ),
        bytes: Bucket::new(
            cfg.max_bytes_per_min as f64,
            cfg.max_bytes_per_min as f64 / 60.0,
        ),
        shared: shared.clone(),
    };

    let _ = conn
        .tx
        .try_send(encode(relay_frame::Kind::Hello(pb::ServerHello {
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            limits: Some(pb::Limits {
                max_frame_bytes: cfg.max_frame_bytes as u32,
                max_channels_per_conn: cfg.max_channels_per_conn as u32,
                max_publish_per_sec: cfg.max_frames_per_sec,
            }),
        })));

    let idle = cfg.idle_timeout;
    let mut shutdown = shared.shutdown.clone();
    loop {
        let next = tokio::select! {
            _ = shutdown.wait_for(|stop| *stop) => break,
            next = timeout(idle, stream.next()) => next,
        };
        match next {
            Ok(Some(Ok(Message::Binary(bytes)))) => conn.handle_frame(&bytes),
            Ok(Some(Ok(Message::Text(_)))) => conn.error(0, ErrorCode::BadFrame),
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            // 닫힘, 오류, 유휴 시간 초과
            _ => break,
        }
    }

    conn.leave_all();
    shared.metrics.connection_closed();
    drop(conn);
    let _ = writer.await;
}

impl Conn {
    fn error(&self, request_id: u32, code: ErrorCode) {
        self.shared.metrics.error(code);
        let _ = self.tx.try_send(encode(relay_frame::Kind::Error(pb::Error {
            request_id,
            code: code as i32,
        })));
    }

    fn handle_frame(&mut self, bytes: &[u8]) {
        self.shared.metrics.frame_in(bytes.len());
        let cfg = &self.shared.config;

        if bytes.len() > cfg.max_frame_bytes {
            // 프레임 안의 request_id를 알아낼 수 있으면 돌려준다.
            let request_id = pb::ClientFrame::decode(bytes)
                .ok()
                .and_then(|f| match f.kind {
                    Some(client_frame::Kind::Publish(p)) => Some(p.request_id),
                    _ => None,
                })
                .unwrap_or(0);
            return self.error(request_id, ErrorCode::TooLarge);
        }
        if !self.frames.take(1.0) || !self.bytes.take(bytes.len() as f64) {
            let request_id = match pb::ClientFrame::decode(bytes).ok().and_then(|f| f.kind) {
                Some(client_frame::Kind::Publish(p)) => p.request_id,
                _ => 0,
            };
            return self.error(request_id, ErrorCode::RateLimited);
        }

        match pb::ClientFrame::decode(bytes).ok().and_then(|f| f.kind) {
            Some(client_frame::Kind::Subscribe(s)) => self.subscribe(s),
            Some(client_frame::Kind::Unsubscribe(u)) => self.unsubscribe(u),
            Some(client_frame::Kind::Publish(p)) => self.publish(p),
            Some(client_frame::Kind::Ping(_)) => {
                let _ = self
                    .tx
                    .try_send(encode(relay_frame::Kind::Pong(pb::Pong {})));
            }
            None => self.error(0, ErrorCode::BadFrame),
        }
    }

    fn subscribe(&mut self, s: pb::Subscribe) {
        let (Ok(channel), Ok(tag)) = (
            <[u8; 32]>::try_from(s.channel_id.as_slice()),
            <[u8; 16]>::try_from(s.device_tag.as_slice()),
        ) else {
            return self.error(0, ErrorCode::BadFrame);
        };
        if !self.channels.contains(&channel)
            && self.channels.len() >= self.shared.config.max_channels_per_conn
        {
            return self.error(0, ErrorCode::TooManyChannels);
        }

        let mut hub = self.shared.hub.lock().unwrap();
        let subs = hub.channels.entry(channel).or_default();
        subs.insert(
            self.id,
            Subscriber {
                tag,
                tx: self.tx.clone(),
            },
        );
        self.channels.insert(channel);
        broadcast_presence(&channel, subs);
    }

    fn unsubscribe(&mut self, u: pb::Unsubscribe) {
        let Ok(channel) = <[u8; 32]>::try_from(u.channel_id.as_slice()) else {
            return self.error(0, ErrorCode::BadFrame);
        };
        if !self.channels.remove(&channel) {
            return self.error(0, ErrorCode::NotSubscribed);
        }
        self.leave(&channel);
    }

    fn publish(&mut self, p: pb::Publish) {
        let Ok(channel) = <[u8; 32]>::try_from(p.channel_id.as_slice()) else {
            return self.error(p.request_id, ErrorCode::BadFrame);
        };
        if !self.channels.contains(&channel) {
            return self.error(p.request_id, ErrorCode::NotSubscribed);
        }

        let targets: HashSet<[u8; 16]> =
            p.to.iter()
                .filter_map(|t| <[u8; 16]>::try_from(t.as_slice()).ok())
                .collect();

        let mut delivered = 0u32;
        {
            let hub = self.shared.hub.lock().unwrap();
            if let Some(subs) = hub.channels.get(&channel) {
                let Some(from) = subs.get(&self.id).map(|s| s.tag) else {
                    return self.error(p.request_id, ErrorCode::NotSubscribed);
                };
                let frame = encode(relay_frame::Kind::Deliver(pb::Deliver {
                    channel_id: channel.to_vec(),
                    from_tag: from.to_vec(),
                    payload: p.payload,
                }));
                for (id, sub) in subs {
                    if *id == self.id || (!targets.is_empty() && !targets.contains(&sub.tag)) {
                        continue;
                    }
                    // 대기열이 가득 찬 느린 수신자는 건너뛴다. 저장하거나 재시도하지 않는다.
                    if sub.tx.try_send(frame.clone()).is_ok() {
                        delivered += 1;
                    }
                }
            }
        }
        self.shared.metrics.delivered(delivered as usize);
        let _ = self.tx.try_send(encode(relay_frame::Kind::Ack(pb::Ack {
            request_id: p.request_id,
            delivered,
            pushed: 0,
        })));
    }

    fn leave(&self, channel: &[u8; 32]) {
        let mut hub = self.shared.hub.lock().unwrap();
        if let Some(subs) = hub.channels.get_mut(channel) {
            subs.remove(&self.id);
            if subs.is_empty() {
                hub.channels.remove(channel);
            } else {
                broadcast_presence(channel, subs);
            }
        }
    }

    fn leave_all(&mut self) {
        for channel in std::mem::take(&mut self.channels) {
            self.leave(&channel);
        }
    }
}

/// 채널의 현재 온라인 태그 목록을 그 채널 구독자 모두에게 알린다.
fn broadcast_presence(channel: &[u8; 32], subs: &std::collections::HashMap<u64, Subscriber>) {
    let frame = encode(relay_frame::Kind::Presence(pb::Presence {
        channel_id: channel.to_vec(),
        online_tags: subs.values().map(|s| s.tag.to_vec()).collect(),
    }));
    for sub in subs.values() {
        let _ = sub.tx.try_send(frame.clone());
    }
}
