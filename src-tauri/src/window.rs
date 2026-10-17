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
pub fn app_ready(window: tauri::WebviewWindow, state: tauri::State<'_, std::sync::Arc<AppState>>) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    hide_webview_when_minimized(&window);
    tracing::info!(elapsed_ms = state.started.elapsed().as_millis() as u64, "window shown");
    Ok(())
}

/// While the window is minimized, tells WebView2 its view is not visible
/// (`ICoreWebView2Controller::put_IsVisible(false)`): the page then reports
/// `visibilityState = "hidden"`, stops drawing frames and throttles its timers, and the UI
/// pauses its progress bars and animations. Restored windows get it back before they
/// are shown. Hooked once, on the first `app_ready` (a page reload calls it again).
fn hide_webview_when_minimized(window: &tauri::WebviewWindow) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static HOOKED: AtomicBool = AtomicBool::new(false);
    if HOOKED.swap(true, Ordering::SeqCst) {
        return;
    }
    let shown = AtomicBool::new(true);
    let w = window.clone();
    window.on_window_event(move |event| {
        // Minimizing and restoring arrive as resizes (to 0×0 and back).
        if !matches!(event, tauri::WindowEvent::Resized(_)) {
            return;
        }
        let visible = !w.is_minimized().unwrap_or(false);
        if shown.swap(visible, Ordering::SeqCst) == visible {
            return;
        }
        set_webview_visible(&w, visible);
    });
}

#[cfg(windows)]
fn set_webview_visible(window: &tauri::WebviewWindow, visible: bool) {
    let res = window.with_webview(move |webview| {
        // SAFETY: a plain COM call on the controller Tauri owns, made on the UI thread
        // that `with_webview` runs us on.
        if let Err(e) = unsafe { webview.controller().SetIsVisible(visible) } {
            tracing::warn!(error = %e, visible, "webview visibility not changed");
        }
    });
    match res {
        Ok(()) => tracing::debug!(visible, "webview visibility"),
        Err(e) => tracing::warn!(error = %e, "with_webview failed"),
    }
}

#[cfg(not(windows))]
fn set_webview_visible(_window: &tauri::WebviewWindow, _visible: bool) {}
