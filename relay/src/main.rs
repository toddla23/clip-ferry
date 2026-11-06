use clap::Parser;
use clip_ferry_relay::auth::{AuthConfig, Secret};
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
    /// 계정 서버 주소 (예: http://127.0.0.1:8788). 주면 접속 토큰이 있는 연결만 받는다 (F33).
    /// 없으면 토큰 없이 누구나 접속한다 (앱에 로그인이 생기기 전까지의 기본).
    #[arg(long, env = "CF_RELAY_ACCOUNT_URL")]
    account_url: Option<String>,
    /// 계정 서버로 사용량을 보낼 때 쓰는 공유 비밀 (계정 서버의 CF_ACCOUNT_USAGE_SECRET과 같게)
    #[arg(long, env = "CF_RELAY_USAGE_SECRET", hide_env_values = true)]
    usage_secret: Option<String>,
    /// 계정당 동시 연결 수
    #[arg(long, env = "CF_RELAY_MAX_CONNS_PER_ACCOUNT", default_value_t = 16)]
    max_conns_per_account: usize,
    /// 계정당 분당 바이트
    #[arg(long, env = "CF_RELAY_MAX_BYTES_PER_MIN_PER_ACCOUNT", default_value_t = 256 * 1024 * 1024)]
    max_bytes_per_min_per_account: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();

    // compose가 빈 값을 넘길 수 있으므로 빈 주소는 "설정 안 함"으로 본다
    let account_url = args.account_url.as_ref().filter(|u| !u.trim().is_empty());
    let auth = account_url.map(|url| AuthConfig {
        usage_secret: args
            .usage_secret
            .clone()
            .filter(|s| !s.is_empty())
            .map(Secret),
        max_conns_per_account: args.max_conns_per_account,
        max_bytes_per_min_per_account: args.max_bytes_per_min_per_account,
        ..AuthConfig::new(url)
    });
    match &auth {
        Some(a) if a.usage_secret.is_none() => {
            tracing::info!("access tokens required (account server set); usage reporting off")
        }
        Some(_) => tracing::info!("access tokens required (account server set)"),
        None => tracing::warn!("no account server: accepting connections without access tokens"),
    }
    let config = Config {
        max_frame_bytes: args.max_frame_bytes,
        max_channels_per_conn: args.max_channels_per_conn,
        max_frames_per_sec: args.max_frames_per_sec,
        max_bytes_per_min: args.max_bytes_per_min,
        max_conns_per_ip: args.max_conns_per_ip,
        trust_proxy: args.trust_proxy,
        auth,
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
