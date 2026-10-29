use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};

use crate::metrics::Metrics;
use crate::Config;

/// 연결로 내보낼 인코딩된 `RelayFrame`.
pub type Outbox = mpsc::Sender<Vec<u8>>;

pub struct Subscriber {
    pub tag: [u8; 16],
    pub tx: Outbox,
}

#[derive(Default)]
pub struct Hub {
    /// 채널 ID → (연결 ID → 구독자). 상태는 이것뿐이다.
    pub channels: HashMap<[u8; 32], HashMap<u64, Subscriber>>,
    ip_conns: HashMap<IpAddr, usize>,
}

pub struct Shared {
    pub config: Config,
    pub hub: Mutex<Hub>,
    pub metrics: Metrics,
    pub shutdown: watch::Receiver<bool>,
    next_conn: AtomicU64,
}

impl Shared {
    pub fn new(config: Config, shutdown: watch::Receiver<bool>) -> Self {
        Self {
            config,
            hub: Mutex::new(Hub::default()),
            metrics: Metrics::default(),
            shutdown,
            next_conn: AtomicU64::new(1),
        }
    }

    pub fn next_conn_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }
}

/// IP당 동시 연결 수를 세는 가드. 드롭되면 자리를 돌려준다.
pub struct IpGuard {
    shared: Arc<Shared>,
    ip: IpAddr,
}

impl IpGuard {
    pub fn acquire(shared: &Arc<Shared>, ip: IpAddr) -> Option<Self> {
        let mut hub = shared.hub.lock().unwrap();
        let n = hub.ip_conns.entry(ip).or_insert(0);
        if *n >= shared.config.max_conns_per_ip {
            return None;
        }
        *n += 1;
        Some(Self {
            shared: shared.clone(),
            ip,
        })
    }
}

impl Drop for IpGuard {
    fn drop(&mut self) {
        let mut hub = self.shared.hub.lock().unwrap();
        if let Some(n) = hub.ip_conns.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                hub.ip_conns.remove(&self.ip);
            }
        }
    }
}
