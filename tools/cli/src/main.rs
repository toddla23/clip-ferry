mod file_store;
mod pair;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use clip_ferry_core::api::{ClipFerry, Config, Status};
use clip_ferry_core::crypto::GroupKey;
use clip_ferry_core::transport::{Client, Event, TransportConfig};
use file_store::FileSecretStore;

/// clip-ferry 개발·검증용 CLI
#[derive(Parser, Debug)]
#[command(name = "clip-ferry-cli", version, about)]
struct Args {
    /// 프로필 이름. 한 컴퓨터에서 여러 기기를 흉내 낼 때 쓴다
    #[arg(long, env = "CF_PROFILE", default_value = "default", global = true)]
    profile: String,
    /// 프로필 폴더들이 들어 있는 곳
    #[arg(
        long,
        env = "CF_PROFILES_DIR",
        default_value = ".cf-profiles",
        global = true
    )]
    profiles_dir: PathBuf,
    /// 릴레이 주소
    #[arg(long, env = "CF_RELAY", global = true)]
    relay: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 이 프로필의 기기 키를 만들고 이름을 정한다
    Init {
        /// 기기 이름
        #[arg(long)]
        name: String,
    },
    /// 그룹 관리
    Group {
        #[command(subcommand)]
        action: GroupAction,
    },
    /// 기기 이름, 에포크, 구성원 수, 릴레이 연결 상태를 보여 준다
    Status,
    /// 새 기기를 초대한다: 코드와 QR을 보여 주고 참여 요청을 기다린다
    Invite,
    /// 초대 코드(또는 링크)로 그룹에 참여한다
    Join {
        /// `invite`가 보여 준 텍스트 코드 또는 링크
        code: String,
    },
    /// 암호화 없이 릴레이만 시험한다 (원시 바이트)
    Debug {
        #[command(subcommand)]
        action: DebugAction,
    },
}

#[derive(Subcommand, Debug)]
enum GroupAction {
    /// 새 그룹을 만든다 (에포크 1, 구성원은 이 기기뿐)
    Create,
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

impl Args {
    fn relay_url(&self) -> String {
        self.relay
            .clone()
            .unwrap_or_else(|| DEFAULT_RELAY.to_string())
    }
}

const DEFAULT_RELAY: &str = "ws://127.0.0.1:8787";

/// 시험용 임의 태그 (`GroupKey` 난수 생성기를 재사용).
pub(crate) fn random_tag() -> [u8; 16] {
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&GroupKey::generate().as_bytes()[..16]);
    tag
}

fn open_profile(args: &Args) -> anyhow::Result<ClipFerry> {
    eprintln!("경고: 개발용 FileSecretStore를 쓰고 있습니다. 비밀이 평문 파일로 저장됩니다.");
    let dir = args.profiles_dir.join(&args.profile);
    ClipFerry::open(
        Config {
            data_dir: dir.clone(),
        },
        Box::new(FileSecretStore::new(dir.join("secrets"))),
    )
    .context("프로필을 열 수 없습니다")
}

/// `status` 출력. 비밀값은 넣지 않고 device_id는 앞 4바이트만 보인다.
fn render_status(status: &Status, relay: &str) -> String {
    let epoch = status
        .epoch
        .map_or("그룹 없음".to_string(), |e| e.to_string());
    format!(
        "기기 이름: {}\ndevice_id: {}\n에포크: {}\n구성원: {}명\n릴레이: {}\n",
        status.device_name,
        status.device_id.short_hex(),
        epoch,
        status.member_count,
        relay
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match &args.command {
        None => Ok(()),
        Some(Command::Init { name }) => {
            let app = open_profile(&args)?;
            app.init_device(name)?;
            println!(
                "초기화됨: device_id {}",
                app.status()?.device_id.short_hex()
            );
            Ok(())
        }
        Some(Command::Group {
            action: GroupAction::Create,
        }) => {
            let app = open_profile(&args)?;
            app.create_group()?;
            println!("그룹 생성됨: 에포크 1");
            Ok(())
        }
        Some(Command::Status) => {
            let app = open_profile(&args)?;
            let status = app.status()?;
            let relay = match app.connection() {
                Some(conn) => {
                    relay_state(&args.relay_url(), conn.channel_id.0, conn.device_tag.0).await
                }
                None => "연결 안 함 (그룹 없음)",
            };
            print!("{}", render_status(&status, relay));
            Ok(())
        }
        Some(Command::Invite) => {
            let app = open_profile(&args)?;
            pair::invite(&app, args.relay_url()).await
        }
        Some(Command::Join { code }) => {
            let app = open_profile(&args)?;
            pair::join(&app, code, args.relay.clone(), DEFAULT_RELAY).await
        }
        Some(Command::Debug { action }) => match action {
            DebugAction::Sub { channel } => {
                debug_sub(&args.relay_url(), parse_channel(channel)?).await
            }
            DebugAction::Pub { channel, message } => {
                debug_pub(&args.relay_url(), parse_channel(channel)?, message.clone()).await
            }
        },
    }
}

/// 릴레이에 붙어 `ServerHello`를 받으면 `connected`.
async fn relay_state(relay: &str, channel: [u8; 32], tag: [u8; 16]) -> &'static str {
    let (client, mut events) = Client::connect(relay, TransportConfig::default());
    client.subscribe(channel, tag);
    let wait = async {
        while let Some(ev) = events.recv().await {
            if matches!(ev, Event::Connected(_)) {
                return true;
            }
        }
        false
    };
    match tokio::time::timeout(Duration::from_secs(3), wait).await {
        Ok(true) => "connected",
        _ => "disconnected",
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

#[cfg(test)]
mod tests {
    use super::*;
    use clip_ferry_core::store::{MemorySecretStore, SecretStore};

    #[test]
    fn status_output_has_no_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = MemorySecretStore::default();
        let app = ClipFerry::open(
            Config {
                data_dir: dir.path().to_path_buf(),
            },
            Box::new(secrets.clone()),
        )
        .unwrap();
        app.init_device("laptop").unwrap();
        app.create_group().unwrap();

        let out = render_status(&app.status().unwrap(), "connected");
        assert!(out.contains("laptop"));
        assert!(out.contains("에포크: 1"));
        assert!(out.contains("구성원: 1명"));

        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let conn = app.connection().unwrap();
        let id = app.status().unwrap().device_id;
        for secret in [
            hex(&secrets.get("device_keys").unwrap()),
            hex(&secrets.get("group_key/1").unwrap()),
            hex(&secrets.get("state_key").unwrap()),
            hex(&conn.channel_id.0),
            hex(&conn.device_tag.0),
            hex(&id.0), // 전체 device_id (짧은 표기만 허용)
        ] {
            assert!(!out.contains(&secret), "{out}");
        }
        assert!(out.contains(&id.short_hex()));
        assert_eq!(id.short_hex().len(), 8);
    }

    #[test]
    fn channel_hex_is_padded() {
        let ch = parse_channel("00ff").unwrap();
        assert_eq!(ch[0], 0);
        assert_eq!(ch[1], 0xff);
        assert!(ch[2..].iter().all(|b| *b == 0));
        assert!(parse_channel("0").is_err());
        assert!(parse_channel("zz").is_err());
        assert!(parse_channel(&"00".repeat(33)).is_err());
    }
}
