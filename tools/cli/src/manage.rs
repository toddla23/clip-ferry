//! `cf devices` / `cf rename` / `cf remove`: 기기 관리.

use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use clip_ferry_core::api::{ClipFerry, DeviceInfo, GroupHealth, Status};
use clip_ferry_core::transport::TransportConfig;

use crate::pair::sanitize;
use crate::text::Printer;

pub fn health_label(health: GroupHealth) -> &'static str {
    match health {
        GroupHealth::Active => "정상",
        GroupHealth::Removed => "removed or stale (그룹에서 제거됨)",
        GroupHealth::Stale => "removed or stale (30일 넘게 연락 없음)",
    }
}

/// 기기 목록. device_id는 앞 4바이트만 보인다.
pub fn render_devices(status: &Status, devices: &[DeviceInfo]) -> String {
    let mut out = format!(
        "기기 {}대 · 에포크 {} · 상태: {}\n",
        devices.len(),
        status.epoch.map_or("-".to_string(), |e| e.to_string()),
        health_label(status.health),
    );
    for d in devices {
        out.push_str(&format!(
            "{} {}  {}{}\n",
            if d.is_self { "*" } else { " " },
            sanitize(&d.name),
            d.device_id.short_hex(),
            if d.is_self { "  (이 기기)" } else { "" },
        ));
    }
    out
}

pub fn devices(app: &ClipFerry) -> anyhow::Result<()> {
    let status = app.status()?;
    print!("{}", render_devices(&status, &app.devices()));
    Ok(())
}

async fn online_session(
    app: &Arc<ClipFerry>,
    relay: &str,
) -> anyhow::Result<clip_ferry_core::session::Session> {
    let (sink, _removed) = Printer::new();
    let session = app.start_session(relay, TransportConfig::default(), sink)?;
    if !session.wait_connected(Duration::from_secs(5)).await {
        bail!("릴레이({relay})에 연결할 수 없습니다");
    }
    Ok(session)
}

pub async fn rename(app: Arc<ClipFerry>, relay: &str, name: &str) -> anyhow::Result<()> {
    let session = online_session(&app, relay).await?;
    let delivered = session.rename(name).await?;
    println!(
        "이름을 바꿨습니다: {} (delivered={delivered})",
        sanitize(name)
    );
    Ok(())
}

pub async fn remove(app: Arc<ClipFerry>, relay: &str, target: &str) -> anyhow::Result<()> {
    let session = online_session(&app, relay).await?;
    let delivered = session.remove_device(target).await?;
    let status = app.status()?;
    println!(
        "제거했습니다: {} · 새 에포크 {} · 구성원 {}명 (delivered={delivered})",
        sanitize(target),
        status.epoch.unwrap_or(0),
        status.member_count,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_ferry_core::crypto::DeviceId;

    #[test]
    fn device_list_shows_only_short_ids() {
        let status = Status {
            device_name: "laptop".into(),
            device_id: DeviceId([1; 16]),
            epoch: Some(2),
            member_count: 2,
            health: GroupHealth::Active,
        };
        let devices = vec![
            DeviceInfo {
                device_id: DeviceId([1; 16]),
                name: "laptop".into(),
                is_self: true,
                added_at_ms: 0,
            },
            DeviceInfo {
                device_id: DeviceId([0xab; 16]),
                name: "pho\x1bne".into(),
                is_self: false,
                added_at_ms: 0,
            },
        ];
        let out = render_devices(&status, &devices);
        assert!(out.starts_with("기기 2대 · 에포크 2 · 상태: 정상\n"));
        assert!(out.contains("* laptop  01010101  (이 기기)"));
        assert!(out.contains("  pho?ne  abababab"));
        assert!(!out.contains("0101010101010101"));
    }

    #[test]
    fn removed_and_stale_are_labelled() {
        assert!(health_label(GroupHealth::Removed).starts_with("removed or stale"));
        assert!(health_label(GroupHealth::Stale).starts_with("removed or stale"));
    }
}
