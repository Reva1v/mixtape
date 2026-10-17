//! Main window: native-looking caption buttons with Windows 11 snap layouts, a deferred
//! first show so the user never sees an unpainted window, hiding to the tray, and a
//! webview that stops drawing whenever the window is out of sight.

use crate::state::AppState;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime, WebviewWindow, plugin::TauriPlugin};

/// Must match `--titlebar-h` in `ui/src/app.css`.
pub const TITLEBAR_HEIGHT: u32 = 40;
pub const MAIN: &str = "main";

/// The window is hidden to the tray (close button, tray icon or a hidden start).
static IN_TRAY: AtomicBool = AtomicBool::new(false);
/// What the webview was last told about its visibility.
static WEBVIEW_SHOWN: AtomicBool = AtomicBool::new(true);
/// When the window last lost focus: pressing the tray icon takes the focus first, so
/// «visible and focused» has to look a moment back.
static BLURRED_AT: Mutex<Option<Instant>> = Mutex::new(None);
const BLUR_GRACE: Duration = Duration::from_millis(500);

pub fn frame_plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_frame::FramePluginBuilder::new()
        .auto_titlebar(true)
        .snap_overlay(true)
        .titlebar_height(TITLEBAR_HEIGHT)
        .button_width(46)
        .button_hover_bg("rgba(255,255,255,0.08)")
        .build()
}

/// Hooks the main window's events. Runs once, from `setup`; `in_tray` = a hidden start,
/// where the window stays hidden until the tray icon or a second launch shows it.
pub fn install(app: &AppHandle, in_tray: bool) {
    let Some(window) = app.get_webview_window(MAIN) else { return };
    if in_tray {
        IN_TRAY.store(true, Ordering::SeqCst);
        sync_webview(&window);
    }
    let w = window.clone();
    window.on_window_event(move |event| match event {
        // Minimizing and restoring arrive as resizes (to 0×0 and back).
        tauri::WindowEvent::Resized(_) => {
            sync_webview(&w);
            crate::mini::on_resized(&w);
        }
        tauri::WindowEvent::Focused(false) => {
            *BLURRED_AT.lock().expect("blur time poisoned") = Some(Instant::now());
        }
        _ => {}
    });
}

/// Called by the UI after its first frame is painted.
#[tauri::command]
pub fn app_ready(window: tauri::WebviewWindow, state: tauri::State<'_, std::sync::Arc<AppState>>) -> Result<(), String> {
    if IN_TRAY.load(Ordering::SeqCst) {
        tracing::info!(elapsed_ms = state.started.elapsed().as_millis() as u64, "ready in the tray");
        return Ok(());
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    tracing::info!(elapsed_ms = state.started.elapsed().as_millis() as u64, "window shown");
    Ok(())
}

/// Brings the window back from the tray, the taskbar or behind other windows.
pub fn show_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else { return };
    IN_TRAY.store(false, Ordering::SeqCst);
    if w.is_minimized().unwrap_or(false) {
        let _ = w.unminimize();
    }
    // The page starts drawing before the window appears.
    sync_webview(&w);
    let _ = w.show();
    let _ = w.set_focus();
}

/// Hides the window to the tray; the music keeps playing and the page stops drawing.
pub fn hide_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else { return };
    IN_TRAY.store(true, Ordering::SeqCst);
    sync_webview(&w);
    let _ = w.hide();
}

/// Tray icon click: hides the window if it is the one in front, shows it otherwise.
pub fn toggle_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else { return };
    let on_screen =
        !IN_TRAY.load(Ordering::SeqCst) && w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false);
    let blurred_just_now =
        BLURRED_AT.lock().expect("blur time poisoned").is_some_and(|t| t.elapsed() < BLUR_GRACE);
    if on_screen && (w.is_focused().unwrap_or(false) || blurred_just_now) {
        hide_main(app);
    } else {
        show_main(app);
    }
}

/// The window's close button: to the tray, or out (the setting «close to tray»).
/// Returns whether the close was turned into hiding.
pub fn close_to_tray(app: &AppHandle) -> bool {
    let Some(st) = app.try_state::<std::sync::Arc<AppState>>() else { return false };
    if !crate::commands::settings::Prefs::load(&st).close_to_tray {
        return false;
    }
    hide_main(app);
    true
}

/// Quits the app (tray menu): saves the queue position, then exits.
pub fn quit(app: &AppHandle) {
    if let Some(st) = app.try_state::<std::sync::Arc<AppState>>() {
        st.player.save_snapshot(true);
        st.player.finish_listens();
    }
    app.exit(0);
}

/// While the window is minimized or in the tray, tells WebView2 its view is not visible
/// (`ICoreWebView2Controller::put_IsVisible(false)`): the page then reports
/// `visibilityState = "hidden"`, stops drawing frames and throttles its timers, and the UI
/// pauses its progress bars and animations. It gets visibility back before the window
/// is shown again.
fn sync_webview(w: &WebviewWindow) {
    let visible = !IN_TRAY.load(Ordering::SeqCst) && !w.is_minimized().unwrap_or(false);
    if WEBVIEW_SHOWN.swap(visible, Ordering::SeqCst) != visible {
        set_webview_visible(w, visible);
    }
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
