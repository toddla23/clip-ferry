//! `cf send` / `cf listen`: 텍스트를 보내고 받는다.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use clip_ferry_core::api::{Clip, ClipFerry};
use clip_ferry_core::session::{ConnectionStatus, EventSink};
use clip_ferry_core::transport::TransportConfig;
use tokio::io::AsyncReadExt;

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

struct Printer;

impl EventSink for Printer {
    fn on_clip(&self, clip: Clip) {
        println!("{}", format_clip(&clip));
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
    let session = app.start_session(relay, TransportConfig::default(), Arc::new(Printer))?;
    if !session.wait_connected(Duration::from_secs(5)).await {
        bail!("릴레이({relay})에 연결할 수 없습니다");
    }
    let delivered = session.send_text(&text).await?;
    println!("delivered={delivered}");
    Ok(())
}

pub async fn listen(app: Arc<ClipFerry>, relay: &str) -> anyhow::Result<()> {
    let _session = app.start_session(relay, TransportConfig::default(), Arc::new(Printer))?;
    tokio::signal::ctrl_c().await?;
    Ok(())
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

    #[tokio::test]
    async fn explicit_text_is_used_as_is() {
        assert_eq!(read_text(Some("hi\n".into())).await.unwrap(), "hi");
        assert_eq!(read_text(Some("hi\r\n".into())).await.unwrap(), "hi");
        assert_eq!(read_text(Some("a\nb".into())).await.unwrap(), "a\nb");
    }
}
