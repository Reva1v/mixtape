//! The Mixtape app as the session's host: its main window, tray and sidecars.

use mixtape_core::Track;
use mixtape_player::PlaybackState;
use mixtape_session::{HandoffEvent, Host, HostKind};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Commands of the app (not of the `mixtape` plugin) the interface may call; `build.rs`
/// lists the same.
const CAPS: &[&str] = &[
    "app_ready",
    "window_set_mini",
    "window_is_mini",
    "tray_menu_snapshot",
    "tray_menu_reveal",
    "tray_menu_hide",
    "tray_menu_action",
    "autostart_get",
    "autostart_set",
    "app_version",
    "update_check",
    "update_install",
];

pub struct AppHost;

impl Host for AppHost {
    fn kind(&self) -> HostKind {
        HostKind::Standalone
    }

    fn ui_window(&self) -> Option<String> {
        Some(crate::window::MAIN.to_owned())
    }

    fn show_ui(&self, app: &AppHandle, _screen: Option<&str>) {
        crate::window::show_main(app);
    }

    fn smtc_hwnd(&self, app: &AppHandle) -> Option<isize> {
        let win = app.get_webview_window(crate::window::MAIN)?;
        win.hwnd().ok().map(|h| h.0 as isize)
    }

    fn taskbar_buttons(&self) -> bool {
        true
    }

    /// yt-dlp and deno sit next to the executable (Tauri `externalBin`); in a debug build
    /// they may still be only in `src-tauri/binaries`.
    fn sidecar(&self, name: &str) -> PathBuf {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
        let beside = exe_dir.join(format!("{name}.exe"));
        if beside.exists() || !cfg!(debug_assertions) {
            return beside;
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries").join(format!("{name}-x86_64-pc-windows-msvc.exe"))
    }

    fn caps(&self) -> Vec<String> {
        CAPS.iter().map(|c| (*c).to_owned()).collect()
    }

    fn on_handoff(&self, _app: &AppHandle, ev: HandoffEvent) {
        tracing::info!(?ev, "hand-off");
    }

    fn on_state(&self, app: &AppHandle, s: &PlaybackState) {
        crate::tray::on_state(app, s);
    }

    fn on_track(&self, app: &AppHandle, t: &Track) {
        crate::tray::on_track(app, t);
    }
}
