use clap::Parser;
use clip_ferry_relay::{spawn, Config};
use tokio::net::TcpListener;

/// clip-ferry 릴레이 서버
#[derive(Parser, Debug)]
#[command(name = "clip-ferry-relay", version, about)]
struct Args {
    /// 듣기 주소
    #[arg(long, env = "CF_RELAY_LISTEN", default_value = "127.0.0.1:8787")]
    listen: String,
    /// 프레임 최대 바이트
    #[arg(long, env = "CF_RELAY_MAX_FRAME_BYTES", default_value_t = 2 * 1024 * 1024)]
    max_frame_bytes: usize,
    /// 연결당 구독 채널 수
    #[arg(long, env = "CF_RELAY_MAX_CHANNELS", default_value_t = 8)]
    max_channels_per_conn: usize,
    /// 연결당 초당 프레임 수
    #[arg(long, env = "CF_RELAY_MAX_FRAMES_PER_SEC", default_value_t = 50)]
    max_frames_per_sec: u32,
    /// 연결당 분당 바이트
    #[arg(long, env = "CF_RELAY_MAX_BYTES_PER_MIN", default_value_t = 64 * 1024 * 1024)]
    max_bytes_per_min: u64,
    /// IP당 동시 연결 수
    #[arg(long, env = "CF_RELAY_MAX_CONNS_PER_IP", default_value_t = 32)]
    max_conns_per_ip: usize,
    /// 리버스 프록시 뒤에서 `X-Forwarded-For`를 믿는다 (루프백·사설 주소에서 직접 온 연결만)
    #[arg(long, env = "CF_RELAY_TRUST_PROXY")]
    trust_proxy: bool,
    /// APNs 인증 키 파일(.p8). 주면 iOS 푸시를 보낸다 (F22). 아래 key-id·team-id도 필요하다
    #[arg(long, env = "CF_RELAY_APNS_KEY")]
    apns_key: Option<std::path::PathBuf>,
    /// APNs 키 ID (Apple 개발자 계정의 Keys 화면)
    #[arg(long, env = "CF_RELAY_APNS_KEY_ID")]
    apns_key_id: Option<String>,
    /// Apple 개발자 팀 ID
    #[arg(long, env = "CF_RELAY_APNS_TEAM_ID")]
    apns_team_id: Option<String>,
    /// iOS 앱 번들 ID
    #[arg(long, env = "CF_RELAY_APNS_TOPIC", default_value = "app.clipferry.ios")]
    apns_topic: String,
    /// 샌드박스 APNs로 보낸다 (Xcode에서 직접 설치한 개발 빌드)
    #[arg(long, env = "CF_RELAY_APNS_SANDBOX")]
    apns_sandbox: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();

    let apns = match (args.apns_key, args.apns_key_id, args.apns_team_id) {
        (Some(path), Some(key_id), Some(team_id)) => Some(clip_ferry_relay::push::ApnsConfig {
            key_pem: std::fs::read(&path)
                .map_err(|e| anyhow::anyhow!("APNs 키 파일을 읽을 수 없습니다: {e}"))?,
            key_id,
            team_id,
            topic: args.apns_topic,
            sandbox: args.apns_sandbox,
        }),
        (None, None, None) => None,
        _ => anyhow::bail!("--apns-key, --apns-key-id, --apns-team-id는 함께 줘야 합니다"),
    };

    let config = Config {
        max_frame_bytes: args.max_frame_bytes,
        max_channels_per_conn: args.max_channels_per_conn,
        max_frames_per_sec: args.max_frames_per_sec,
        max_bytes_per_min: args.max_bytes_per_min,
        max_conns_per_ip: args.max_conns_per_ip,
        trust_proxy: args.trust_proxy,
        apns,
        ..Config::default()
    };
    let listener = TcpListener::bind(&args.listen).await?;
    let handle = spawn(listener, config);
    tracing::info!("listening on {}", handle.local_addr());

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    handle.shutdown().await;
    Ok(())
}
