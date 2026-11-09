//! clip-ferry 계정 서버 (F32, D-019).
//!
//! 구글 ID 토큰을 검증해 계정을 만들고, 릴레이 접속 토큰(JWT, Ed25519)과 갱신 토큰을 발급한다.
//! 저장하는 것은 계정 기본정보·기기 목록·갱신 토큰 해시·일별 사용량뿐이다.
//! 클립보드 내용과 그룹 키는 이 서버를 지나지 않는다.

pub mod api;
pub mod google;
pub mod link;
pub mod store;
pub mod tokens;

use std::time::{SystemTime, UNIX_EPOCH};

/// 지금 시각 (Unix 밀리초).
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 무작위 16바이트를 hex 32자로. 계정 ID·토큰 묶음 ID에 쓴다.
pub fn random_id() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("OS 난수를 얻지 못했습니다");
    hex::encode(b)
}
