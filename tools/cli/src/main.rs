use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use clip_ferry_core::crypto::GroupKey;
use clip_ferry_core::transport::{Client, Event, TransportConfig};

/// clip-ferry 개발·검증용 CLI
#[derive(Parser, Debug)]
#[command(name = "clip-ferry-cli", version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 암호화 없이 릴레이만 시험한다 (원시 바이트)
    Debug {
        /// 릴레이 주소
        #[arg(
            long,
            env = "CF_RELAY",
            default_value = "ws://127.0.0.1:8787",
            global = true
        )]
        relay: String,
        #[command(subcommand)]
        action: DebugAction,
    },
}

#[derive(Subcommand, Debug)]
enum DebugAction {
    /// 채널을 구독하고 받은 내용을 한 줄씩 출력한다
    Sub {
        /// 채널 ID (hex, 32바이트보다 짧으면 뒤를 0으로 채움)
        channel: String,
    },
    /// 채널에 한 번 게시하고 전달된 수를 출력한다
    Pub {
        /// 채널 ID (hex, 32바이트보다 짧으면 뒤를 0으로 채움)
        channel: String,
        /// 보낼 문자열
        message: String,
    },
}

fn parse_channel(hex: &str) -> anyhow::Result<[u8; 32]> {
    if hex.len() % 2 != 0 || !hex.is_ascii() {
        bail!("채널은 짝수 길이의 hex여야 합니다: {hex}");
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("hex가 아닙니다: {hex}"))?;
    if bytes.len() > 32 {
        bail!("채널은 최대 32바이트입니다");
    }
    let mut channel = [0u8; 32];
    channel[..bytes.len()].copy_from_slice(&bytes);
    Ok(channel)
}

/// 시험용 임의 태그 (`GroupKey` 난수 생성기를 재사용).
fn random_tag() -> [u8; 16] {
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&GroupKey::generate().as_bytes()[..16]);
    tag
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.command {
        None => Ok(()),
        Some(Command::Debug { relay, action }) => match action {
            DebugAction::Sub { channel } => debug_sub(&relay, parse_channel(&channel)?).await,
            DebugAction::Pub { channel, message } => {
                debug_pub(&relay, parse_channel(&channel)?, message).await
            }
        },
    }
}

async fn debug_sub(relay: &str, channel: [u8; 32]) -> anyhow::Result<()> {
    let (client, mut events) = Client::connect(relay, TransportConfig::default());
    client.subscribe(channel, random_tag());
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            ev = events.recv() => match ev {
                None => return Ok(()),
                Some(Event::Connected(_)) => eprintln!("connected"),
                Some(Event::Reconnecting { .. }) => eprintln!("reconnecting..."),
                Some(Event::Deliver(d)) => println!("{}", String::from_utf8_lossy(&d.payload)),
                Some(Event::Error(e)) => eprintln!("error: {:?}", e.code()),
                Some(_) => {}
            },
        }
    }
}

async fn debug_pub(relay: &str, channel: [u8; 32], message: String) -> anyhow::Result<()> {
    let (client, mut events) = Client::connect(relay, TransportConfig::default());
    // 릴레이는 구독 중인 채널에만 게시를 허용한다.
    client.subscribe(channel, random_tag());
    let mut request_id = None;
    loop {
        match events.recv().await.context("연결이 종료됨")? {
            Event::Connected(_) => {
                request_id = Some(client.publish(channel, message.clone().into_bytes(), vec![]));
            }
            Event::Reconnecting { .. } => eprintln!("reconnecting..."),
            Event::Ack(a) if Some(a.request_id) == request_id => {
                println!("delivered={}", a.delivered);
                return Ok(());
            }
            Event::Error(e) => bail!("릴레이 오류: {:?}", e.code()),
            _ => {}
        }
    }
}
