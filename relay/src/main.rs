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
    #[arg(long, env = "CF_RELAY_MAX_FRAME_BYTES", default_value_t = 512 * 1024)]
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();

    let config = Config {
        max_frame_bytes: args.max_frame_bytes,
        max_channels_per_conn: args.max_channels_per_conn,
        max_frames_per_sec: args.max_frames_per_sec,
        max_bytes_per_min: args.max_bytes_per_min,
        max_conns_per_ip: args.max_conns_per_ip,
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
