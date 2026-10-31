//! `cf send` / `cf listen`: 텍스트를 보내고 받는다.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use clip_ferry_core::api::{Clip, ClipFerry, GroupHealth, ImageClip};
use clip_ferry_core::session::{ConnectionStatus, EventSink};
use clip_ferry_core::transfer::{sniff, MAX_IMAGE_BYTES};
use clip_ferry_core::transport::TransportConfig;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use crate::pair::sanitize;

/// 받은 내용을 터미널에 찍기 전에 제어 문자를 지운다(줄바꿈·탭은 둔다).
/// 상대가 보낸 값이므로 화면을 조작하는 이스케이프 시퀀스를 막는다.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && !matches!(c, '\n' | '\t') {
                '?'
            } else {
                c
            }
        })
        .collect()
}

pub fn format_clip(clip: &Clip) -> String {
    format!("[{}] {}", sanitize(&clip.from_name), printable(&clip.text))
}

pub const REMOVED_NOTICE: &str =
    "removed or stale: 그룹에서 제거되었거나 너무 오래 연결되지 않았습니다. 다시 페어링하세요";

/// 터미널에 상태와 받은 텍스트를 찍는다. 그룹에서 제거된 것을 알면 `removed` 채널로 알린다.
pub struct Printer {
    removed: mpsc::UnboundedSender<()>,
}

impl Printer {
    pub fn new() -> (Arc<Printer>, mpsc::UnboundedReceiver<()>) {
        let (removed, rx) = mpsc::unbounded_channel();
        (Arc::new(Printer { removed }), rx)
    }
}

impl EventSink for Printer {
    fn on_clip(&self, clip: Clip) {
        println!("{}", format_clip(&clip));
    }

    fn on_image(&self, image: ImageClip) {
        let from = sanitize(&image.from_name);
        match save_image(Path::new(RECEIVED_DIR), &image) {
            Ok(path) => println!(
                "[{from}] 이미지 {} ({}x{}, {} bytes) -> {}",
                image.mime,
                image.width,
                image.height,
                image.bytes.len(),
                path.display()
            ),
            Err(e) => eprintln!("[{from}] 받은 이미지를 저장하지 못했습니다: {e}"),
        }
    }

    fn on_status(&self, status: ConnectionStatus) {
        match status {
            ConnectionStatus::Connected => eprintln!("connected"),
            ConnectionStatus::Reconnecting => eprintln!("reconnecting..."),
            ConnectionStatus::Disconnected => {}
        }
    }

    fn on_devices_changed(&self) {
        eprintln!("기기 목록이 바뀌었습니다");
    }

    fn on_epoch_changed(&self, epoch: u64) {
        eprintln!("키가 회전되었습니다 (에포크 {epoch})");
    }

    fn on_group_health(&self, health: GroupHealth) {
        match health {
            GroupHealth::Active => {}
            GroupHealth::Stale => eprintln!("경고: {REMOVED_NOTICE}"),
            GroupHealth::Removed => {
                eprintln!("{REMOVED_NOTICE}");
                let _ = self.removed.send(());
            }
        }
    }
}

/// 받은 이미지를 저장하는 폴더 (현재 폴더 아래). CLI 전용 디버그 기능이다.
const RECEIVED_DIR: &str = "received";

fn extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// 파일 이름에 쓸 수 있게 영문·숫자·`-`·`_` 외의 글자는 `_`로 바꾼다 (보낸 사람의 이름은 믿지 않는다).
fn file_safe(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(32)
        .collect();
    if s.is_empty() {
        "device".into()
    } else {
        s
    }
}

/// `dir/<시각>-<보낸 기기>.<확장자>`로 저장한다. 이미 있으면 번호를 붙인다.
fn save_image(dir: &Path, image: &ImageClip) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).context("received 폴더를 만들 수 없습니다")?;
    let base = format!("{}-{}", image.created_at_ms, file_safe(&image.from_name));
    let ext = extension(&image.mime);
    for n in 0.. {
        let name = if n == 0 {
            format!("{base}.{ext}")
        } else {
            format!("{base}-{n}.{ext}")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(&image.bytes)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!()
}

/// 이미지 파일을 읽어 그룹의 다른 기기들에게 보낸다.
pub async fn send_image(app: Arc<ClipFerry>, relay: &str, path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("{}을(를) 읽을 수 없습니다", path.display()))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        bail!("이미지가 너무 큽니다 ({} bytes, 최대 20 MiB)", bytes.len());
    }
    let Some(info) = sniff(&bytes) else {
        bail!("PNG, JPEG, GIF, WebP 이미지가 아닙니다");
    };
    let (sink, _removed) = Printer::new();
    let session = app.start_session(relay, TransportConfig::default(), sink)?;
    if !session.wait_connected(Duration::from_secs(5)).await {
        bail!("릴레이({relay})에 연결할 수 없습니다");
    }
    session.wait_online(2, Duration::from_millis(1500)).await;
    let delivered = session
        .send_image(&bytes, info.mime, info.width, info.height)
        .await?;
    println!("delivered={delivered}");
    Ok(())
}

/// 인자로 받은 텍스트, 없으면 표준 입력. 입력 끝의 줄바꿈 하나는 지운다.
async fn read_text(arg: Option<String>) -> anyhow::Result<String> {
    let mut text = match arg {
        Some(t) if t != "-" => t,
        _ => {
            let mut buf = String::new();
            tokio::io::stdin()
                .read_to_string(&mut buf)
                .await
                .context("표준 입력을 읽을 수 없습니다")?;
            buf
        }
    };
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    Ok(text)
}

pub async fn send(app: Arc<ClipFerry>, relay: &str, text: Option<String>) -> anyhow::Result<()> {
    let text = read_text(text).await?;
    let (sink, _removed) = Printer::new();
    let session = app.start_session(relay, TransportConfig::default(), sink)?;
    if !session.wait_connected(Duration::from_secs(5)).await {
        bail!("릴레이({relay})에 연결할 수 없습니다");
    }
    // 다른 기기가 막 새 키로 넘어오는 중일 수 있으니 붙을 때까지 잠깐(최대 1.5초) 기다린다.
    session.wait_online(2, Duration::from_millis(1500)).await;
    let delivered = session.send_text(&text).await?;
    println!("delivered={delivered}");
    Ok(())
}

pub async fn listen(app: Arc<ClipFerry>, relay: &str) -> anyhow::Result<()> {
    let (sink, mut removed) = Printer::new();
    let _session = app.start_session(relay, TransportConfig::default(), sink)?;
    tokio::select! {
        r = tokio::signal::ctrl_c() => r.map_err(Into::into),
        // 제거되면 더 받을 수 없으므로 알리고 끝낸다.
        _ = removed.recv() => bail!("더 이상 이 그룹의 메시지를 받을 수 없습니다"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_ferry_core::crypto::DeviceId;

    #[test]
    fn terminal_escapes_are_neutralised() {
        assert_eq!(printable("a\x1b[31mred\x07\x08"), "a?[31mred??");
        assert_eq!(printable("줄1\n\t줄2"), "줄1\n\t줄2");
    }

    #[test]
    fn clip_line_format() {
        let clip = Clip {
            from: DeviceId([0; 16]),
            from_name: "lap\x1btop".into(),
            text: "안녕 clip-ferry".into(),
            created_at_ms: 0,
        };
        assert_eq!(format_clip(&clip), "[lap?top] 안녕 clip-ferry");
    }

    fn image(from: &str) -> ImageClip {
        ImageClip {
            from: DeviceId([0; 16]),
            from_name: from.into(),
            mime: "image/png".into(),
            bytes: vec![1, 2, 3],
            width: 1,
            height: 1,
            created_at_ms: 42,
        }
    }

    #[test]
    fn sender_names_cannot_escape_the_received_folder() {
        assert_eq!(file_safe("../../etc/passwd"), "______etc_passwd");
        assert_eq!(file_safe("a b\\c:d"), "a_b_c_d");
        assert_eq!(file_safe(""), "device");
        assert_eq!(file_safe(&"x".repeat(100)).len(), 32);
        let dir = tempfile::tempdir().unwrap();
        let path = save_image(dir.path(), &image("../evil")).unwrap();
        assert_eq!(path.parent().unwrap(), dir.path());
        assert_eq!(path.file_name().unwrap(), "42-___evil.png");
    }

    #[test]
    fn saved_images_never_overwrite_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let a = save_image(dir.path(), &image("pc")).unwrap();
        let b = save_image(dir.path(), &image("pc")).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), [1, 2, 3]);
        assert_eq!(b.file_name().unwrap(), "42-pc-1.png");
    }

    #[tokio::test]
    async fn explicit_text_is_used_as_is() {
        assert_eq!(read_text(Some("hi\n".into())).await.unwrap(), "hi");
        assert_eq!(read_text(Some("hi\r\n".into())).await.unwrap(), "hi");
        assert_eq!(read_text(Some("a\nb".into())).await.unwrap(), "a\nb");
    }
}
