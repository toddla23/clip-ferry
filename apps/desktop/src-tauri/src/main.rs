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
async fn set_relay_url(shell: State<'_, Arc<Shell>>, url: String) -> Result<AppView, String> {
    shell.set_relay_url(&url)
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_view, set_relay_url])
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

            let data_dir = app.path().app_data_dir()?;
            let shell = Shell::new(handle, data_dir, Box::new(secrets::KeyringStore::new()))?;
            app.manage(shell.clone());

            tauri::async_runtime::spawn(async move {
                match shell.ensure_group() {
                    Ok(()) => shell.start(),
                    Err(e) => shell.fail(&e),
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("clip-ferry 데스크톱 앱을 시작하지 못했습니다");
}
