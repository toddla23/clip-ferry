//! 트레이 아이콘: 연결 상태를 색으로 보여 주고, 열기/종료 메뉴를 둔다.

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::view::{AppView, Connection, Health, Stage};

const TRAY_ID: &str = "main";
const SIZE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Busy,
    Off,
    Problem,
}

pub fn tone(view: &AppView) -> Tone {
    // 아직 그룹이 없다 (첫 실행 마법사 중)
    if view.stage == Stage::Setup {
        return Tone::Off;
    }
    if view.health != Health::Active {
        return Tone::Problem;
    }
    match view.connection {
        Connection::Connected => Tone::Ok,
        Connection::Reconnecting => Tone::Busy,
        Connection::Disconnected => Tone::Off,
        Connection::Stopped => Tone::Problem,
    }
}

fn rgb(tone: Tone) -> [u8; 3] {
    match tone {
        Tone::Ok => [22, 163, 74],
        Tone::Busy => [217, 119, 6],
        Tone::Off => [156, 163, 175],
        Tone::Problem => [220, 38, 38],
    }
}

/// 투명 배경 위의 색 원 (RGBA).
pub fn circle_rgba(tone: Tone) -> Vec<u8> {
    let [r, g, b] = rgb(tone);
    let c = SIZE as f32 / 2.0;
    let radius = c - 2.0;
    let mut px = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            // 가장자리 1픽셀은 부드럽게.
            let a = (radius + 0.5 - d).clamp(0.0, 1.0);
            px.extend_from_slice(&[r, g, b, (a * 255.0) as u8]);
        }
    }
    px
}

fn icon(tone: Tone) -> Image<'static> {
    Image::new_owned(circle_rgba(tone), SIZE, SIZE)
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "열기", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(Tone::Busy))
        .tooltip("clip-ferry")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 상태가 바뀔 때마다 아이콘 색과 툴팁을 맞춘다.
pub fn update(app: &AppHandle, view: &AppView) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let label = if view.stage == Stage::Setup {
        "설정 필요"
    } else if view.health == Health::Active {
        view.connection.label()
    } else {
        "다시 페어링 필요"
    };
    let _ = tray.set_icon(Some(icon(tone(view))));
    let _ = tray.set_tooltip(Some(format!("clip-ferry — {label}")));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(connection: Connection, health: Health) -> AppView {
        AppView {
            stage: Stage::Ready,
            connection,
            health,
            ..AppView::default()
        }
    }

    #[test]
    fn tone_follows_connection_and_health() {
        assert_eq!(tone(&view(Connection::Connected, Health::Active)), Tone::Ok);
        assert_eq!(
            tone(&view(Connection::Reconnecting, Health::Active)),
            Tone::Busy
        );
        assert_eq!(
            tone(&view(Connection::Disconnected, Health::Active)),
            Tone::Off
        );
        // 제거되었으면 연결돼 있어도 문제로 표시한다.
        assert_eq!(
            tone(&view(Connection::Connected, Health::Removed)),
            Tone::Problem
        );
    }

    #[test]
    fn setup_is_gray_even_before_any_connection() {
        let v = AppView {
            stage: Stage::Setup,
            connection: Connection::Reconnecting,
            ..AppView::default()
        };
        assert_eq!(tone(&v), Tone::Off);
    }

    #[test]
    fn circle_is_colored_in_the_middle_and_transparent_in_the_corner() {
        let px = circle_rgba(Tone::Ok);
        assert_eq!(px.len(), (SIZE * SIZE * 4) as usize);
        let at = |x: u32, y: u32| &px[((y * SIZE + x) * 4) as usize..][..4];
        assert_eq!(at(16, 16), &[22, 163, 74, 255]);
        assert_eq!(at(0, 0)[3], 0);
    }

    #[test]
    fn each_tone_has_a_different_color() {
        let colors: Vec<_> = [Tone::Ok, Tone::Busy, Tone::Off, Tone::Problem]
            .into_iter()
            .map(rgb)
            .collect();
        for (i, a) in colors.iter().enumerate() {
            assert!(colors[i + 1..].iter().all(|b| a != b));
        }
    }
}
