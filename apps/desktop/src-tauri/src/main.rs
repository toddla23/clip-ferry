// 릴리스 빌드에서 Windows 콘솔 창이 뜨지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod secrets;
mod settings;
mod shell;
mod tray;
mod view;

use std::sync::Arc;

use tauri::{Manager, State, WindowEvent};

use shell::Shell;
use view::AppView;

#[tauri::command]
fn get_view(shell: State<'_, Arc<Shell>>) -> AppView {
    shell.view()
}

#[tauri::command]
async fn create_group(shell: State<'_, Arc<Shell>>, name: String) -> Result<AppView, String> {
    shell.create_group(&name)
}

#[tauri::command]
async fn start_join(
    shell: State<'_, Arc<Shell>>,
    name: String,
    code: String,
) -> Result<AppView, String> {
    shell.start_join(&name, &code)
}

#[tauri::command]
async fn start_invite(shell: State<'_, Arc<Shell>>) -> Result<AppView, String> {
    shell.start_invite()
}

#[tauri::command]
fn decide_invite(shell: State<'_, Arc<Shell>>, approve: bool) {
    shell.decide_invite(approve);
}

#[tauri::command]
fn close_pairing(shell: State<'_, Arc<Shell>>) -> AppView {
    shell.close_pairing()
}

#[tauri::command]
async fn set_relay_url(shell: State<'_, Arc<Shell>>, url: String) -> Result<AppView, String> {
    shell.set_relay_url(&url)
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_view,
            set_relay_url,
            create_group,
            start_join,
            start_invite,
            decide_invite,
            close_pairing
        ])
        .on_window_event(|window, event| {
            // 창을 닫아도 트레이에 남는다. 종료는 트레이 메뉴에서.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            tray::build(&handle)?;

            // 개발용: 환경 변수 CF_DESKTOP_PROFILE=<이름>이면 데이터 폴더와 키 저장소 이름을
            // 따로 쓴다. 한 컴퓨터에서 앱을 두 개 띄워 페어링을 시험할 때 쓴다.
            let mut data_dir = app.path().app_data_dir()?;
            let mut keys = secrets::KeyringStore::new();
            if let Some(profile) = settings::dev_profile() {
                data_dir = data_dir.join("profiles").join(&profile);
                keys = secrets::KeyringStore::with_service(&format!("clip-ferry-{profile}"));
            }
            let shell = Shell::new(handle, data_dir, Box::new(keys))?;
            app.manage(shell.clone());

            tauri::async_runtime::spawn(async move {
                shell.begin();
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("clip-ferry 데스크톱 앱을 시작하지 못했습니다");
}
