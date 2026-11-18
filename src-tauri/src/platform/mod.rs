use crate::models::{AgentProvider, AgentSession, AppSettings, HudMode, ToastCorner};
use std::process::Command;
use std::sync::Mutex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, Position, Size, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

#[cfg(target_os = "macos")]
mod antigravity;

pub const CAPSULE_W: f64 = 236.0;
pub const CAPSULE_H: f64 = 64.0;
pub const ISLAND_W: f64 = 268.0;
pub const ISLAND_H: f64 = 52.0;
pub const PANEL_W: f64 = 404.0;
pub const PANEL_H: f64 = 528.0;

pub const TOAST_W: f64 = 384.0;
pub const TOAST_H: f64 = 280.0;

/// Remembers the pre-expand capsule logical coordinates so collapsing restores the exact spot
static PRE_EXPAND_POS: Mutex<Option<(f64, f64)>> = Mutex::new(None);

fn nonactivating_builder<'a>(
    app: &'a AppHandle,
    label: &str,
    url: WebviewUrl,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    #[cfg(target_os = "macos")]
    {
        tao::platform::macos::with_nonactivating_panel(|| WebviewWindowBuilder::new(app, label, url))
    }
    #[cfg(not(target_os = "macos"))]
    {
        WebviewWindowBuilder::new(app, label, url)
    }
}

pub fn apply_native_floating_traits(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let native_window = window.clone();
        let _ = window.run_on_main_thread(move || {
            use objc2_app_kit::{NSColor, NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};
            if let Ok(raw_ns_win) = native_window.ns_window() {
                unsafe {
                    let ns_win = &*(raw_ns_win as *const NSWindow);
                    // The HUD and corner alerts can overlap. Keep alerts clickable when
                    // the HUD is open without activating either panel.
                    let level = NSStatusWindowLevel + isize::from(native_window.label() == "toast");
                    ns_win.setLevel(level);
                    ns_win.setCollectionBehavior(
                        NSWindowCollectionBehavior::CanJoinAllSpaces
                            | NSWindowCollectionBehavior::FullScreenAuxiliary
                            | NSWindowCollectionBehavior::Stationary,
                    );
                    ns_win.setHidesOnDeactivate(false);
                    ns_win.setOpaque(false);
                    let clear = NSColor::clearColor();
                    ns_win.setBackgroundColor(Some(&clear));
                    ns_win.setHasShadow(false);
                }
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        };
        if let Ok(raw_hwnd) = window.hwnd() {
            let hwnd = raw_hwnd.0 as *mut core::ffi::c_void;
            unsafe {
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    ex | (WS_EX_NOACTIVATE as isize) | (WS_EX_TOOLWINDOW as isize),
                );
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }
}

fn primary_logical_bounds(app: &AppHandle) -> (f64, f64) {
    if let Ok(Some(mon)) = app.primary_monitor() {
        let scale = mon.scale_factor();
        let size = mon.size();
        return (size.width as f64 / scale, size.height as f64 / scale);
    }
    (1440.0, 900.0)
}

pub fn create_floating_windows(app: &AppHandle, settings: &AppSettings) -> tauri::Result<()> {
    let (screen_w, screen_h) = primary_logical_bounds(app);

    let (init_w, init_h, init_x, init_y) = match settings.hud_mode {
        HudMode::TopIsland => (
            ISLAND_W,
            ISLAND_H,
            ((screen_w - ISLAND_W) / 2.0).max(16.0),
            6.0,
        ),
        HudMode::Capsule => {
            let (x, y) = settings
                .capsule_pos
                .unwrap_or_else(|| ((screen_w - CAPSULE_W - 24.0).max(24.0), 76.0));
            (
                CAPSULE_W,
                CAPSULE_H,
                x.clamp(8.0, (screen_w - CAPSULE_W - 8.0).max(8.0)),
                y.clamp(28.0, (screen_h - CAPSULE_H - 24.0).max(28.0)),
            )
        }
    };

    let hud = nonactivating_builder(app, "hud", WebviewUrl::App("index.html".into()))
        .title("AgentCue HUD")
        .inner_size(init_w, init_h)
        .position(init_x, init_y)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .focused(false)
        .accept_first_mouse(true)
        .visible(true)
        .build()?;

    apply_native_floating_traits(&hud);

    let (tx, ty) = toast_logical_position(screen_w, screen_h, settings.toast_corner);
    let toast = nonactivating_builder(app, "toast", WebviewUrl::App("index.html".into()))
        .title("AgentCue Toast")
        .inner_size(TOAST_W, TOAST_H)
        .position(tx, ty)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .focused(false)
        .accept_first_mouse(true)
        .visible(true)
        .build()?;

    apply_native_floating_traits(&toast);
    // Start click-through so invisible Toast corner never intercepts mouse clicks
    let _ = toast.set_ignore_cursor_events(true);

    Ok(())
}

fn toast_logical_position(screen_w: f64, screen_h: f64, corner: ToastCorner) -> (f64, f64) {
    let x = (screen_w - TOAST_W - 16.0).max(16.0);
    let y = match corner {
        ToastCorner::TopRight => 38.0,
        ToastCorner::BottomRight => (screen_h - TOAST_H - 48.0).max(38.0),
    };
    (x, y)
}

pub fn prepare_toast_window(app: &AppHandle, settings: &AppSettings) {
    if let Some(toast) = app.get_webview_window("toast") {
        let (screen_w, screen_h) = primary_logical_bounds(app);
        let (tx, ty) = toast_logical_position(screen_w, screen_h, settings.toast_corner);
        let _ = toast.set_position(Position::Logical(LogicalPosition::new(tx, ty)));
        let _ = toast.show();
        let _ = toast.set_ignore_cursor_events(false);
        apply_native_floating_traits(&toast);
    }
}

pub fn set_toast_interactive(app: &AppHandle, interactive: bool) {
    if let Some(toast) = app.get_webview_window("toast") {
        let _ = toast.set_ignore_cursor_events(!interactive);
    }
}

pub fn snap_capsule_position(app: &AppHandle, x: f64, y: f64) -> (f64, f64) {
    const SNAP_DISTANCE: f64 = 28.0;
    let Some(hud) = app.get_webview_window("hud") else {
        return (x, y);
    };
    let (screen_w, _) = primary_logical_bounds(app);
    let right = (screen_w - CAPSULE_W - 8.0).max(8.0);
    let target_x = if (x - 8.0).abs() <= SNAP_DISTANCE {
        8.0
    } else if (x - right).abs() <= SNAP_DISTANCE {
        right
    } else {
        x
    };
    let target_y = if (y - 28.0).abs() <= SNAP_DISTANCE { 28.0 } else { y };

    if ((x - target_x).abs() > 0.5 || (y - target_y).abs() > 0.5)
        && hud.set_position(Position::Logical(LogicalPosition::new(target_x, target_y))).is_ok()
    {
        return (target_x, target_y);
    }
    (x, y)
}

pub fn resize_hud_for_state(app: &AppHandle, expanded: bool, settings: &AppSettings) {
    let Some(hud) = app.get_webview_window("hud") else {
        return;
    };
    let (screen_w, screen_h) = primary_logical_bounds(app);
    let scale = hud.scale_factor().unwrap_or(2.0);
    let current_logical = hud
        .outer_position()
        .ok()
        .map(|p| (p.x as f64 / scale, p.y as f64 / scale));

    if expanded {
        if let Some((cx, cy)) = current_logical {
            let mut saved_pos = PRE_EXPAND_POS.lock().unwrap();
            if saved_pos.is_none()
                && settings.hud_mode == HudMode::Capsule
                && hud.inner_size().is_ok_and(|size| {
                    (size.width as f64 / scale - CAPSULE_W).abs() < 1.0
                })
            {
                *saved_pos = Some((cx, cy));
            }
            let (cx, cy) = saved_pos.or(settings.capsule_pos).unwrap_or((cx, cy));
            let target_x = match settings.hud_mode {
                HudMode::TopIsland => ((screen_w - PANEL_W) / 2.0).max(12.0),
                HudMode::Capsule => {
                    // Anchor right edge if capsule is on right half of screen
                    let anchor_right = cx > screen_w * 0.5;
                    let raw_x = if anchor_right {
                        cx + CAPSULE_W - PANEL_W
                    } else {
                        cx
                    };
                    raw_x.clamp(10.0, (screen_w - PANEL_W - 10.0).max(10.0))
                }
            };
            let target_y = match settings.hud_mode {
                HudMode::TopIsland => 6.0,
                HudMode::Capsule => cy.clamp(28.0, (screen_h - PANEL_H - 20.0).max(28.0)),
            };
            let _ = hud.set_size(Size::Logical(LogicalSize::new(PANEL_W, PANEL_H)));
            let _ = hud.set_position(Position::Logical(LogicalPosition::new(target_x, target_y)));
        } else {
            let _ = hud.set_size(Size::Logical(LogicalSize::new(PANEL_W, PANEL_H)));
        }
    } else {
        let (target_w, target_h) = match settings.hud_mode {
            HudMode::TopIsland => (ISLAND_W, ISLAND_H),
            HudMode::Capsule => (CAPSULE_W, CAPSULE_H),
        };
        let saved = PRE_EXPAND_POS.lock().unwrap().take();
        let (target_x, target_y) = match settings.hud_mode {
            HudMode::TopIsland => (((screen_w - ISLAND_W) / 2.0).max(16.0), 6.0),
            HudMode::Capsule => saved
                .or(settings.capsule_pos)
                .unwrap_or(((screen_w - CAPSULE_W - 24.0).max(24.0), 76.0)),
        };
        let _ = hud.set_size(Size::Logical(LogicalSize::new(target_w, target_h)));
        let _ = hud.set_position(Position::Logical(LogicalPosition::new(
            target_x.clamp(8.0, (screen_w - target_w - 8.0).max(8.0)),
            target_y.clamp(6.0, (screen_h - target_h - 16.0).max(6.0)),
        )));
    }
    apply_native_floating_traits(&hud);
}

pub fn reset_hud_position(app: &AppHandle, settings: &AppSettings) {
    *PRE_EXPAND_POS.lock().unwrap() = None;
    if let Some(hud) = app.get_webview_window("hud") {
        let (screen_w, _screen_h) = primary_logical_bounds(app);
        let (w, h, x, y) = match settings.hud_mode {
            HudMode::TopIsland => (ISLAND_W, ISLAND_H, ((screen_w - ISLAND_W) / 2.0).max(16.0), 6.0),
            HudMode::Capsule => (CAPSULE_W, CAPSULE_H, (screen_w - CAPSULE_W - 24.0).max(24.0), 76.0),
        };
        let _ = hud.set_size(Size::Logical(LogicalSize::new(w, h)));
        let _ = hud.set_position(Position::Logical(LogicalPosition::new(x, y)));
        let _ = hud.show();
        apply_native_floating_traits(&hud);
    }
}

/// Inspects the ancestor process chain from a hook's client/parent PID to identify the user's Terminal or IDE
pub fn detect_host_from_pid(start_pid: u32, env_term: Option<&str>) -> (Option<u32>, Option<String>) {
    if let Some(term) = env_term {
        let normalized = match term.to_ascii_lowercase().as_str() {
            "ghostty" => Some("Ghostty"),
            "iterm.app" | "iterm2" => Some("iTerm"),
            "warpterminal" | "warp" => Some("Warp"),
            "apple_terminal" => Some("Terminal"),
            "wezterm" => Some("WezTerm"),
            "vscode" => Some("Visual Studio Code"),
            "cursor" => Some("Cursor"),
            _ => None,
        };
        if let Some(app_name) = normalized {
            return (Some(start_pid), Some(app_name.to_string()));
        }
    }

    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing(),
    );

    let known_hosts: &[(&str, &str)] = &[
        ("antigravity", "Antigravity"),
        ("cursor", "Cursor"),
        ("code", "Visual Studio Code"),
        ("ghostty", "Ghostty"),
        ("iterm2", "iTerm"),
        ("warp", "Warp"),
        ("wezterm-gui", "WezTerm"),
        ("alacritty", "Alacritty"),
        ("kitty", "kitty"),
        ("terminal", "Terminal"),
        ("windowsterminal", "Windows Terminal"),
        ("chatgpt", "Codex"),
        ("codex", "Codex"),
        ("claude", "Claude"),
        ("zed", "Zed"),
    ];

    let mut cur = Pid::from_u32(start_pid);
    for _ in 0..12 {
        let Some(proc_info) = sys.process(cur) else {
            break;
        };
        let pname = proc_info.name().to_string_lossy().to_ascii_lowercase();
        for (needle, friendly) in known_hosts {
            if pname == *needle || pname.starts_with(&format!("{needle}.")) {
                // Skip bare CLI `codex` / `claude` if there is a parent GUI terminal above it
                if (*needle == "codex" || *needle == "claude")
                    && proc_info.parent().is_some_and(|pid| pid.as_u32() > 1)
                {
                    continue;
                }
                return (Some(cur.as_u32()), Some((*friendly).to_string()));
            }
        }
        match proc_info.parent() {
            Some(parent_pid) if parent_pid != cur && parent_pid.as_u32() > 1 => {
                cur = parent_pid;
            }
            _ => break,
        }
    }

    (Some(start_pid), None)
}

fn desktop_session_url(
    provider: AgentProvider,
    host_app: Option<&str>,
    session_id: &str,
) -> Option<String> {
    if provider != AgentProvider::Codex || host_app != Some("Codex") {
        return None;
    }
    let thread_id = uuid::Uuid::parse_str(session_id).ok()?;
    Some(format!("codex://threads/{thread_id}"))
}

pub fn open_tracked_session(session: &AgentSession) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if session.provider == AgentProvider::Antigravity {
        return antigravity::open_conversation(&session.raw_session_id);
    }

    let url = desktop_session_url(
        session.provider,
        session.host_app.as_deref(),
        &session.raw_session_id,
    )
    .ok_or("此会话没有可用的精确跳转入口")?;

    #[cfg(target_os = "macos")]
    let opened = Command::new("open")
        .arg(&url)
        .status()
        .is_ok_and(|status| status.success());

    #[cfg(target_os = "windows")]
    let opened = {
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let wide_url: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        let result = unsafe {
            ShellExecuteW(null_mut(), null(), wide_url.as_ptr(), null(), null(), SW_SHOWNORMAL)
        };
        result as isize > 32
    };

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opened = {
        let _ = url;
        false
    };

    opened.then_some(()).ok_or_else(|| "系统无法打开对应的 Codex 会话".into())
}

#[cfg(test)]
#[test]
fn desktop_session_links_require_a_codex_desktop_thread_id() {
    let id = "7f4048f5-e497-4911-9ab6-8be48f7c9778";
    assert_eq!(
        desktop_session_url(AgentProvider::Codex, Some("Codex"), id),
        Some(format!("codex://threads/{id}"))
    );
    assert!(desktop_session_url(AgentProvider::Codex, Some("Codex"), "new").is_none());
    assert!(desktop_session_url(AgentProvider::Codex, Some("Ghostty"), id).is_none());
    assert!(desktop_session_url(AgentProvider::Antigravity, Some("Antigravity"), id).is_none());
}

pub fn open_folder_in_os(cwd: &str) -> Result<(), String> {
    if cwd.trim().is_empty() {
        return Err("目录路径为空".into());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(cwd)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(cwd)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(cwd)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
