//! clip-ferry 공용 코어.

pub mod crypto;
pub mod envelope;
pub mod transport;

/// `proto/clip_ferry.proto`에서 생성한 와이어 타입.
#[allow(clippy::all)]
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/clip_ferry.v1.rs"));
}

/// 코어 크레이트 버전.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
