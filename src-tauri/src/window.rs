//! Main window: native-looking caption buttons with Windows 11 snap layouts, and a
//! deferred first show so the user never sees an unpainted window.

use crate::state::AppState;
use tauri::{Runtime, plugin::TauriPlugin};

/// Must match `--titlebar-h` in `ui/src/app.css`.
pub const TITLEBAR_HEIGHT: u32 = 40;

pub fn frame_plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_frame::FramePluginBuilder::new()
        .auto_titlebar(true)
        .snap_overlay(true)
        .titlebar_height(TITLEBAR_HEIGHT)
        .button_width(46)
        .button_hover_bg("rgba(255,255,255,0.08)")
        .build()
}

/// Called by the UI after its first frame is painted.
#[tauri::command]
pub fn app_ready(window: tauri::WebviewWindow, state: tauri::State<'_, AppState>) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    tracing::info!(elapsed_ms = state.started.elapsed().as_millis() as u64, "window shown");
    Ok(())
}
