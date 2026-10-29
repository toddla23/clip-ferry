use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use clip_ferry_core::pb::ErrorCode;

const ERROR_SLOTS: usize = 8;

/// 집계 카운터. 채널·기기·내용에 대한 정보는 담지 않는다.
#[derive(Default)]
pub struct Metrics {
    connections: AtomicU64,
    rejected: AtomicU64,
    open: AtomicU64,
    frames_in: AtomicU64,
    bytes_in: AtomicU64,
    delivered: AtomicU64,
    errors: [AtomicU64; ERROR_SLOTS],
}

/// 특정 시점의 카운터 값.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub connections: u64,
    pub rejected: u64,
    pub open: u64,
    pub frames_in: u64,
    pub bytes_in: u64,
    pub delivered: u64,
    /// `ErrorCode` 값(0..8) 순서대로.
    pub errors: [u64; ERROR_SLOTS],
}

impl Metrics {
    pub fn connection_opened(&self) {
        self.connections.fetch_add(1, Relaxed);
        self.open.fetch_add(1, Relaxed);
    }

    pub fn connection_closed(&self) {
        self.open.fetch_sub(1, Relaxed);
    }

    pub fn connections_rejected(&self) {
        self.rejected.fetch_add(1, Relaxed);
    }

    pub fn frame_in(&self, bytes: usize) {
        self.frames_in.fetch_add(1, Relaxed);
        self.bytes_in.fetch_add(bytes as u64, Relaxed);
    }

    pub fn delivered(&self, n: usize) {
        self.delivered.fetch_add(n as u64, Relaxed);
    }

    pub fn error(&self, code: ErrorCode) {
        if let Some(slot) = self.errors.get(code as usize) {
            slot.fetch_add(1, Relaxed);
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            connections: self.connections.load(Relaxed),
            rejected: self.rejected.load(Relaxed),
            open: self.open.load(Relaxed),
            frames_in: self.frames_in.load(Relaxed),
            bytes_in: self.bytes_in.load(Relaxed),
            delivered: self.delivered.load(Relaxed),
            errors: std::array::from_fn(|i| self.errors[i].load(Relaxed)),
        }
    }
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "connections={} open={} rejected={} frames_in={} bytes_in={} delivered={} errors={:?}",
            self.connections,
            self.open,
            self.rejected,
            self.frames_in,
            self.bytes_in,
            self.delivered,
            self.errors
        )
    }
}
