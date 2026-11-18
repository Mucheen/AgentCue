pub mod collectors;
pub mod hooks_installer;
pub mod models;
pub mod platform;
pub mod state;

use crate::models::{AgentProvider, AppSettings, DashboardSnapshot};
use crate::state::AppState;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State,
};

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> DashboardSnapshot {
    state.snapshot()
}

#[tauri::command]
fn set_hud_expanded(app: AppHandle, state: State<'_, AppState>, expanded: bool) {
    let settings = state.settings();
    platform::resize_hud_for_state(&app, expanded, &settings);
}

#[tauri::command]
fn save_capsule_pos(app: AppHandle, state: State<'_, AppState>, x: f64, y: f64) {
    let (x, y) = platform::snap_capsule_position(&app, x, y);
    state.save_capsule_pos(x, y);
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
    is_expanded: bool,
) -> DashboardSnapshot {
    let snap = state.update_settings(&app, settings.clone());
    platform::resize_hud_for_state(&app, is_expanded, &settings);
    snap
}

#[tauri::command]
fn install_hooks(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: AgentProvider,
) -> Result<DashboardSnapshot, String> {
    let hook_bin = hooks_installer::resolve_hook_binary_path(state.home_dir());
    hooks_installer::install_provider_hooks(state.home_dir(), provider, &hook_bin)?;
    let snap = state.snapshot();
    let _ = app.emit("sessions-updated", &snap);
    Ok(snap)
}

#[tauri::command]
fn uninstall_hooks(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: AgentProvider,
) -> Result<DashboardSnapshot, String> {
    hooks_installer::uninstall_provider_hooks(state.home_dir(), provider)?;
    let snap = state.snapshot();
    let _ = app.emit("sessions-updated", &snap);
    Ok(snap)
}

#[tauri::command]
async fn jump_to_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let session = state.session(&session_id).ok_or("这条会话已不在列表中")?;
    tauri::async_runtime::spawn_blocking(move || platform::open_tracked_session(&session))
        .await
        .map_err(|_| "会话跳转未能执行".to_string())?
}

#[tauri::command]
fn open_session_folder(cwd: String) -> Result<(), String> {
    platform::open_folder_in_os(&cwd)
}

#[tauri::command]
fn dismiss_session(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> DashboardSnapshot {
    state.dismiss_session(&app, &session_id)
}

#[tauri::command]
fn set_toast_interactive(app: AppHandle, interactive: bool) {
    platform::set_toast_interactive(&app, interactive);
}

fn setup_tray(app: &AppHandle, state: AppState) -> tauri::Result<()> {
    let toggle_item = MenuItem::with_id(app, "toggle_hud", "显示 / 隐藏悬浮窗", true, None::<&str>)?;
    let reset_item = MenuItem::with_id(app, "reset_pos", "重置悬浮窗位置", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 AgentCue", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&toggle_item, &reset_item, &sep, &quit_item])?;

    let icon = Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("AgentCue · Agent 实时进度监控")
        .menu(&menu)
        .show_menu_on_left_click(false);

    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }

    builder
        .on_menu_event(move |app_handle, event| match event.id.as_ref() {
            "toggle_hud" => {
                if let Some(hud) = app_handle.get_webview_window("hud") {
                    if hud.is_visible().unwrap_or(true) {
                        let _ = hud.hide();
                    } else {
                        let _ = hud.show();
                        platform::apply_native_floating_traits(&hud);
                    }
                }
            }
            "reset_pos" => {
                let settings = state.settings();
                platform::reset_hud_position(app_handle, &settings);
                let _ = app_handle.emit("hud-force-collapse", ());
            }
            "quit" => {
                app_handle.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app_handle = tray.app_handle();
                if let Some(hud) = app_handle.get_webview_window("hud") {
                    let _ = hud.show();
                    platform::apply_native_floating_traits(&hud);
                    let _ = app_handle.emit("hud-toggle-expand", ());
                }
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = AppState::new();

    let mut builder = tauri::Builder::default();

    builder = builder.manage(app_state.clone()).setup(move |app| {
        #[cfg(target_os = "macos")]
        {
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        }

        let handle = app.handle().clone();
        let settings = app_state.settings();

        platform::create_floating_windows(&handle, &settings)?;
        setup_tray(&handle, app_state.clone())?;

        collectors::hook_server::spawn_hook_server(handle.clone(), app_state.clone());
        collectors::passive_watcher::spawn_passive_watcher(handle, app_state.clone());

        Ok(())
    });

    builder
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            set_hud_expanded,
            save_capsule_pos,
            update_settings,
            install_hooks,
            uninstall_hooks,
            jump_to_session,
            open_session_folder,
            dismiss_session,
            set_toast_interactive,
        ])
        .run(tauri::generate_context!())
        .expect("error while running agent-ing tauri application");
}
