// 릴리스 빌드에서 Windows 콘솔 창이 뜨지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod clipsync;
mod imgcodec;
mod secrets;
mod settings;
mod shell;
mod sysclip;
mod tray;
mod view;

use std::sync::Arc;

use tauri::{Manager, State, WindowEvent};

use clip_ferry_core::api::{ClipFerry, Config, Error};
use shell::Shell;
use view::AppView;

const RESET_PROMPT: &str = "clip-ferry가 이 컴퓨터에 저장한 정보의 일부가 없거나 손상되어 열 수 없습니다. (앱 데이터 폴더가 지워졌을 수 있습니다.)

[예]: 이 컴퓨터의 그룹 정보를 지우고 처음부터 시작합니다. 시작 화면에서 '이 계정의 다른 기기로 연결'을 누르면 다시 들어옵니다.
[아니요]: 아무것도 지우지 않고 종료합니다.";

/// 로컬 상태를 지우고 처음부터 시작할지 묻는다. `true`면 지운다.
#[cfg(windows)]
fn ask_reset_local(detail: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNO,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let text = wide(&format!(
        "{RESET_PROMPT}

({detail})"
    ));
    let title = wide("clip-ferry");
    // SAFETY: 두 문자열은 NUL로 끝나고 호출이 끝날 때까지 살아 있다.
    let answer = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNO | MB_ICONWARNING | MB_SETFOREGROUND,
        )
    };
    answer == IDYES
}

/// 창을 띄울 방법이 없는 플랫폼에서는 지우지 않고 이유를 남긴 뒤 종료한다.
#[cfg(not(windows))]
fn ask_reset_local(detail: &str) -> bool {
    eprintln!(
        "{RESET_PROMPT}
({detail})"
    );
    false
}

#[tauri::command]
fn get_view(shell: State<'_, Arc<Shell>>) -> AppView {
    shell.view()
}

#[tauri::command]
async fn create_group(shell: State<'_, Arc<Shell>>, name: String) -> Result<AppView, String> {
    shell.create_group(&name)
}

#[tauri::command]
async fn start_link_join(shell: State<'_, Arc<Shell>>, name: String) -> Result<AppView, String> {
    shell.inner().start_link_join(&name).await
}

/// 처음부터 다시 시작 (F44): 이 계정의 다른 기기를 예전 그룹으로 돌리고 이 기기에서 새 그룹을 만든다.
#[tauri::command]
async fn restart_group(shell: State<'_, Arc<Shell>>, name: String) -> Result<AppView, String> {
    shell.inner().restart_group(&name).await
}

#[tauri::command]
async fn accept_link_request(shell: State<'_, Arc<Shell>>) -> Result<AppView, String> {
    shell.accept_link_request()
}

#[tauri::command]
fn dismiss_link_request(shell: State<'_, Arc<Shell>>) -> AppView {
    shell.dismiss_link_request()
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

#[tauri::command]
async fn rename_device(shell: State<'_, Arc<Shell>>, name: String) -> Result<AppView, String> {
    shell.inner().rename_device(&name).await
}

#[tauri::command]
async fn remove_device(shell: State<'_, Arc<Shell>>, id: String) -> Result<AppView, String> {
    shell.remove_device(&id).await
}

#[tauri::command]
fn leave_group(shell: State<'_, Arc<Shell>>) -> Result<AppView, String> {
    shell.leave_group()
}

#[tauri::command]
fn set_sync(shell: State<'_, Arc<Shell>>, send: bool, receive: bool) -> Result<AppView, String> {
    shell.set_sync(send, receive)
}

#[tauri::command]
fn set_paused(shell: State<'_, Arc<Shell>>, paused: bool) -> AppView {
    shell.set_paused(paused)
}

#[tauri::command]
fn start_google_login(shell: State<'_, Arc<Shell>>) -> AppView {
    shell.start_google_login()
}

#[tauri::command]
fn cancel_login(shell: State<'_, Arc<Shell>>) -> AppView {
    shell.cancel_login()
}

#[tauri::command]
async fn dev_login(shell: State<'_, Arc<Shell>>, email: String) -> Result<AppView, String> {
    #[cfg(debug_assertions)]
    {
        shell.dev_login(&email).await
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (shell, email);
        Err("개발용 로그인은 개발 빌드에서만 됩니다".into())
    }
}

#[tauri::command]
async fn logout(shell: State<'_, Arc<Shell>>) -> Result<AppView, String> {
    Ok(shell.logout().await)
}

#[tauri::command]
fn show_server_settings(shell: State<'_, Arc<Shell>>) -> Result<AppView, String> {
    shell.show_server_settings()
}

#[tauri::command]
fn set_account_url(shell: State<'_, Arc<Shell>>, url: String) -> Result<AppView, String> {
    shell.set_account_url(&url)
}

#[tauri::command]
fn set_autostart(shell: State<'_, Arc<Shell>>, on: bool) -> Result<AppView, String> {
    shell.set_autostart(on)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_view,
            set_relay_url,
            create_group,
            restart_group,
            start_link_join,
            accept_link_request,
            dismiss_link_request,
            decide_invite,
            close_pairing,
            rename_device,
            remove_device,
            leave_group,
            set_sync,
            set_paused,
            set_autostart,
            start_google_login,
            cancel_login,
            dev_login,
            logout,
            set_account_url,
            show_server_settings
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
            let mut service = "clip-ferry".to_string();
            if let Some(profile) = settings::dev_profile() {
                data_dir = data_dir.join("profiles").join(&profile);
                service = format!("clip-ferry-{profile}");
            }
            // 계정 로그인 정보(갱신 토큰)도 같은 보안 저장소에 다른 이름으로 둔다.
            let open = || {
                Shell::new(
                    handle.clone(),
                    data_dir.clone(),
                    Box::new(secrets::KeyringStore::with_service(&service)),
                    Box::new(secrets::KeyringStore::with_service(&service)),
                )
            };
            let shell = match open() {
                Ok(shell) => shell,
                // 앱 데이터만 지워졌거나 손상됨: 되살릴 수 없다. 창 없이 꺼지지 않고 물어본다 (F44)
                Err(e @ (Error::StateInconsistent | Error::StateCorrupt)) => {
                    if !ask_reset_local(&e.to_string()) {
                        std::process::exit(1);
                    }
                    ClipFerry::reset_local(
                        &Config {
                            data_dir: data_dir.clone(),
                        },
                        &secrets::KeyringStore::with_service(&service),
                    )?;
                    open()?
                }
                Err(e) => return Err(e.into()),
            };
            app.manage(shell.clone());

            tauri::async_runtime::spawn(async move {
                shell.begin();
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("clip-ferry 데스크톱 앱을 시작하지 못했습니다");
}
