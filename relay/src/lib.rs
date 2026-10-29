//! clip-ferry 릴레이: 채널 단위로 바이트를 전달하는 무저장 중계.
//!
//! 상태는 메모리의 `채널 ID → 연결 목록` 맵뿐이며 디스크에 아무것도 쓰지 않는다.
//! 로그에는 집계 숫자만 남기고 채널 ID·태그·페이로드는 절대 남기지 않는다.

mod conn;
mod hub;
mod limits;
mod metrics;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use conn::run_connection;
use hub::{IpGuard, Shared};

pub use metrics::Snapshot;

/// WebSocket 서브프로토콜 이름.
pub const SUBPROTOCOL: &str = clip_ferry_core::transport::SUBPROTOCOL;

/// 제한값 (docs/02-architecture.md의 기본값).
#[derive(Debug, Clone)]
pub struct Config {
    pub max_frame_bytes: usize,
    pub max_channels_per_conn: usize,
    pub max_frames_per_sec: u32,
    pub max_bytes_per_min: u64,
    pub max_conns_per_ip: usize,
    /// 이 시간 동안 클라이언트에서 아무것도 오지 않으면 연결을 닫는다.
    pub idle_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_frame_bytes: 2 * 1024 * 1024,
            max_channels_per_conn: 8,
            max_frames_per_sec: 50,
            max_bytes_per_min: 64 * 1024 * 1024,
            max_conns_per_ip: 32,
            idle_timeout: Duration::from_secs(60),
        }
    }
}

/// 실행 중인 릴레이.
pub struct RelayHandle {
    addr: SocketAddr,
    shared: Arc<Shared>,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RelayHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn metrics(&self) -> Snapshot {
        self.shared.metrics.snapshot()
    }

    /// 열린 연결을 모두 닫고 멈춘다.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.task.await;
    }
}

/// `listener`에서 릴레이를 시작한다.
pub fn spawn(listener: TcpListener, config: Config) -> RelayHandle {
    let addr = listener.local_addr().expect("바인드된 리스너");
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let shared = Arc::new(Shared::new(config, shutdown_rx.clone()));

    let app = Router::new()
        .route("/", get(ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(shared.clone());

    let mut graceful = shutdown_rx;
    let task = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = graceful.wait_for(|stop| *stop).await;
        })
        .await;
    });

    let log_shared = shared.clone();
    let mut log_stop = shared.shutdown.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => tracing::info!("{}", log_shared.metrics.snapshot()),
                _ = log_stop.wait_for(|stop| *stop) => return,
            }
        }
    });

    RelayHandle {
        addr,
        shared,
        shutdown_tx,
        task,
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(shared): State<Arc<Shared>>,
) -> Response {
    let Some(guard) = IpGuard::acquire(&shared, peer.ip()) else {
        shared.metrics.connections_rejected();
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    // 한도의 2배까지는 연결을 유지한 채 `TOO_LARGE`로 응답하고, 그보다 크면 연결을 끊는다.
    let limit = shared.config.max_frame_bytes.saturating_mul(2);
    ws.protocols([SUBPROTOCOL])
        .max_message_size(limit)
        .max_frame_size(limit)
        .on_upgrade(move |socket| run_connection(socket, shared, guard))
}
