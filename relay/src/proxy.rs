//! 리버스 프록시(예: `tailscale serve`) 뒤에서 실제 클라이언트 IP를 구한다.
//!
//! 프록시 뒤에서는 모든 연결이 프록시 주소로 보여 IP당 연결 제한을 모두가 나눠 쓰게 된다.
//! `trust_proxy`가 켜져 있고 직접 연결한 상대가 루프백·사설 주소일 때만 `X-Forwarded-For`를 믿는다.
//! 인터넷에서 바로 온 연결이 헤더를 위조해 제한을 피하지 못하게 하기 위해서다.

use std::net::IpAddr;

use axum::http::HeaderMap;

/// IP당 제한에 쓸 클라이언트 주소.
pub fn client_ip(peer: IpAddr, headers: &HeaderMap, trust_proxy: bool) -> IpAddr {
    if !trust_proxy || !is_local_proxy(peer) {
        return peer;
    }
    forwarded_for(headers).unwrap_or(peer)
}

/// 가장 가까운 신뢰 프록시가 붙인 값(가장 오른쪽)을 쓴다. 앞쪽 값은 클라이언트가 넣을 수 있다.
fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    let value = headers.get("x-forwarded-for")?.to_str().ok()?;
    value.rsplit(',').next()?.trim().parse().ok()
}

/// 같은 컴퓨터 또는 사설망에서 직접 연결한 상대인가 (Docker 포트 게시는 브리지 주소로 보인다).
fn is_local_proxy(peer: IpAddr) -> bool {
    match peer {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", value.parse().unwrap());
        h
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn off_by_default_ignores_the_header() {
        let peer = ip("127.0.0.1");
        assert_eq!(client_ip(peer, &headers("100.68.109.21"), false), peer);
    }

    #[test]
    fn trusted_loopback_peer_uses_the_forwarded_address() {
        let got = client_ip(ip("127.0.0.1"), &headers("100.68.109.21"), true);
        assert_eq!(got, ip("100.68.109.21"));
    }

    #[test]
    fn docker_bridge_and_private_peers_are_trusted() {
        for peer in ["172.18.0.1", "10.0.0.5", "192.168.1.9", "fd00::1", "::1"] {
            let got = client_ip(ip(peer), &headers("100.68.109.21"), true);
            assert_eq!(got, ip("100.68.109.21"), "{peer}");
        }
    }

    #[test]
    fn public_peer_cannot_spoof_the_header() {
        let peer = ip("203.0.113.5");
        assert_eq!(client_ip(peer, &headers("100.68.109.21"), true), peer);
    }

    #[test]
    fn the_rightmost_entry_wins_so_a_client_cannot_prepend_a_fake() {
        let got = client_ip(ip("127.0.0.1"), &headers("1.2.3.4, 100.68.109.21"), true);
        assert_eq!(got, ip("100.68.109.21"));
    }

    #[test]
    fn missing_or_garbage_header_falls_back_to_the_peer() {
        let peer = ip("127.0.0.1");
        assert_eq!(client_ip(peer, &HeaderMap::new(), true), peer);
        assert_eq!(client_ip(peer, &headers("not-an-ip"), true), peer);
        assert_eq!(client_ip(peer, &headers("100.1.1.1,"), true), peer);
    }
}
