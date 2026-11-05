//! 푸시 게이트웨이 (F22, 03-protocol 5절 "릴레이 규칙").
//!
//! - `Publish.push` 대상 중 **지금 그 채널에 그 태그로 붙어 있지 않은 것만** 푸시를 보낸다.
//! - 토큰은 보내고 바로 잊는다. 저장·재시도·로그 없음(집계 숫자만).
//! - 푸시 페이로드는 봉투(`payload`)를 base64로 담는다. 3,000바이트를 넘으면 봉투 없이
//!   "도착 알림"만 보낸다(`need_open: true`). 받는 기기는 앱을 열어 보낸 기기에 재전송을 요청한다.
//! - 지금은 APNs만 보낸다. FCM(F20)은 보류(D-017)라 FCM 대상은 세지 않고 넘긴다.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use clip_ferry_core::pb;

/// 푸시 페이로드에 봉투를 담을 수 있는 최대 크기(base64로 바꾼 뒤).
pub const MAX_INLINE_B64: usize = 3000;
/// `Publish` 하나에 붙일 수 있는 푸시 대상 수. 넘는 것은 무시한다.
pub const MAX_TARGETS: usize = 16;
/// 토큰 최대 길이. APNs 토큰은 64자 16진수다.
const MAX_TOKEN: usize = 200;

/// 보낼 푸시 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub platform: pb::Platform,
    pub token: String,
    /// 보낸 기기의 태그 (받는 기기가 누구에게 재전송을 요청할지 안다)
    pub from_tag: [u8; 16],
    /// 봉투. 너무 크면 `None`(도착 알림만)
    pub payload: Option<Vec<u8>>,
}

/// 푸시를 실제로 보내는 쪽. 막지 않고 바로 돌아와야 한다(보내기는 백그라운드에서).
pub trait PushSender: Send + Sync {
    fn send(&self, notice: Notice);
}

/// `Publish.push`에서 보낼 푸시를 고른다. `online`은 지금 채널에 붙은 태그들,
/// `to`가 비어 있지 않으면 그 대상에게만 보낸다.
pub fn select(
    targets: &[pb::PushTarget],
    online: &[[u8; 16]],
    to: &[[u8; 16]],
    from_tag: [u8; 16],
    payload: &[u8],
) -> Vec<Notice> {
    let inline = (payload.len().div_ceil(3) * 4 <= MAX_INLINE_B64).then(|| payload.to_vec());
    targets
        .iter()
        .take(MAX_TARGETS)
        .filter_map(|t| {
            let tag = <[u8; 16]>::try_from(t.device_tag.as_slice()).ok()?;
            if online.contains(&tag) || (!to.is_empty() && !to.contains(&tag)) {
                return None;
            }
            let platform = pb::Platform::try_from(t.platform).ok()?;
            if platform != pb::Platform::Apns || !valid_token(&t.token) {
                return None;
            }
            Some(Notice {
                platform,
                token: t.token.clone(),
                from_tag,
                payload: inline.clone(),
            })
        })
        .collect()
}

/// URL 경로에 들어가므로 16진수만 받는다.
fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// APNs 요청 본문(JSON). 알림 문구는 일반적인 말이고, NSE가 복호화해 바꾼다.
pub fn apns_body(notice: &Notice) -> String {
    let from = B64.encode(notice.from_tag);
    let data = match &notice.payload {
        Some(p) => format!(r#""p":"{}""#, B64.encode(p)),
        None => r#""need_open":true"#.to_string(),
    };
    format!(
        concat!(
            r#"{{"aps":{{"alert":{{"title":"clip-ferry","body":"새 클립이 도착했습니다"}},"#,
            r#""mutable-content":1,"sound":"default","thread-id":"clip"}},"f":"{}",{}}}"#
        ),
        from, data
    )
}

/// APNs 설정 (토큰 기반 인증, `.p8`).
#[derive(Clone)]
pub struct ApnsConfig {
    /// `.p8` 파일 내용 (PEM, PKCS#8)
    pub key_pem: Vec<u8>,
    pub key_id: String,
    pub team_id: String,
    /// 앱 번들 ID (`apns-topic`)
    pub topic: String,
    /// 개발 빌드(Xcode에서 직접 설치)는 샌드박스 서버로 보낸다.
    pub sandbox: bool,
}

impl std::fmt::Debug for ApnsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApnsConfig")
            .field("key_id", &self.key_id)
            .field("team_id", &self.team_id)
            .field("topic", &self.topic)
            .field("sandbox", &self.sandbox)
            .finish_non_exhaustive()
    }
}

/// HTTP/2로 APNs에 보낸다. 인증 토큰(JWT)은 50분마다 새로 만든다(APNs는 20~60분 사이를 요구).
pub struct Apns {
    config: ApnsConfig,
    key: jsonwebtoken::EncodingKey,
    client: reqwest::Client,
    jwt: Mutex<Option<(String, u64)>>,
    stats: Arc<crate::metrics::Metrics>,
}

const JWT_REFRESH_SECS: u64 = 50 * 60;

impl Apns {
    pub fn new(config: ApnsConfig, stats: Arc<crate::metrics::Metrics>) -> anyhow::Result<Self> {
        let key = jsonwebtoken::EncodingKey::from_ec_pem(&config.key_pem)
            .map_err(|_| anyhow::anyhow!("APNs 키(.p8)를 읽을 수 없습니다"))?;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            config,
            key,
            client,
            jwt: Mutex::new(None),
            stats,
        })
    }

    fn bearer(&self) -> Option<String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        let mut slot = self.jwt.lock().unwrap();
        if let Some((jwt, at)) = slot.as_ref() {
            if now.saturating_sub(*at) < JWT_REFRESH_SECS {
                return Some(jwt.clone());
            }
        }
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(self.config.key_id.clone());
        #[derive(serde::Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            iat: u64,
        }
        let claims = Claims {
            iss: &self.config.team_id,
            iat: now,
        };
        let jwt = jsonwebtoken::encode(&header, &claims, &self.key).ok()?;
        *slot = Some((jwt.clone(), now));
        Some(jwt)
    }

    fn url(&self, token: &str) -> String {
        let host = if self.config.sandbox {
            "api.sandbox.push.apple.com"
        } else {
            "api.push.apple.com"
        };
        format!("https://{host}/3/device/{token}")
    }
}

impl PushSender for Arc<Apns> {
    fn send(&self, notice: Notice) {
        let Some(bearer) = self.bearer() else {
            self.stats.push_failed();
            return;
        };
        let this = self.clone();
        tokio::spawn(async move {
            let request = this
                .client
                .post(this.url(&notice.token))
                .header("authorization", format!("bearer {bearer}"))
                .header("apns-topic", &this.config.topic)
                .header("apns-push-type", "alert")
                .header("apns-priority", "10")
                // 한 시간 안에 못 전하면 버린다(받는 쪽도 과거 1시간까지만 받는다).
                .header("apns-expiration", (unix_secs() + 3600).to_string())
                .header("content-type", "application/json")
                .body(apns_body(&notice));
            match request.send().await {
                Ok(r) if r.status().is_success() => this.stats.pushed(),
                // 이유(상태 코드)만 센다. 토큰·내용은 남기지 않는다.
                Ok(r) => {
                    this.stats.push_failed();
                    tracing::debug!("apns status {}", r.status().as_u16());
                }
                Err(_) => this.stats.push_failed(),
            }
        });
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(tag: u8, platform: pb::Platform, token: &str) -> pb::PushTarget {
        pb::PushTarget {
            device_tag: vec![tag; 16],
            platform: platform as i32,
            token: token.into(),
        }
    }

    #[test]
    fn only_offline_apns_targets_with_hex_tokens() {
        let targets = [
            target(1, pb::Platform::Apns, "ab12"), // 오프라인 → 보냄
            target(2, pb::Platform::Apns, "cd34"), // 온라인 → 안 보냄
            target(3, pb::Platform::Fcm, "ef56"),  // FCM은 보류
            target(4, pb::Platform::Apns, "../x"), // 16진수가 아님
        ];
        let got = select(&targets, &[[2; 16]], &[], [9; 16], b"env");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].token, "ab12");
        assert_eq!(got[0].from_tag, [9; 16]);
        assert_eq!(got[0].payload.as_deref(), Some(&b"env"[..]));

        // `to`가 있으면 그 대상만.
        let got = select(&targets, &[], &[[4; 16]], [9; 16], b"env");
        assert!(got.is_empty());
        let got = select(&targets, &[], &[[1; 16]], [9; 16], b"env");
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn large_payload_becomes_arrival_notice() {
        let targets = [target(1, pb::Platform::Apns, "ab")];
        let small = vec![0u8; 2250]; // base64 3000자
        let big = vec![0u8; 2251];
        assert!(select(&targets, &[], &[], [0; 16], &small)[0]
            .payload
            .is_some());
        assert!(select(&targets, &[], &[], [0; 16], &big)[0]
            .payload
            .is_none());
    }

    #[test]
    fn body_is_json_with_payload_or_need_open() {
        let mut n = Notice {
            platform: pb::Platform::Apns,
            token: "ab".into(),
            from_tag: [0; 16],
            payload: Some(vec![1, 2, 3]),
        };
        let body = apns_body(&n);
        assert!(body.contains(r#""mutable-content":1"#));
        assert!(body.contains(r#""p":"AQID""#));
        assert!(body.contains(r#""f":"AAAAAAAAAAAAAAAAAAAAAA==""#));
        n.payload = None;
        let body = apns_body(&n);
        assert!(body.contains(r#""need_open":true"#));
        assert!(!body.contains(r#""p""#));
        assert_eq!(body.matches('{').count(), body.matches('}').count());
    }

    #[test]
    fn too_many_targets_are_capped() {
        let targets: Vec<_> = (0..40)
            .map(|i| target(i, pb::Platform::Apns, "ab"))
            .collect();
        assert_eq!(select(&targets, &[], &[], [0; 16], b"x").len(), MAX_TARGETS);
    }
}
