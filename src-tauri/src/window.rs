//! Main window: native-looking caption buttons with Windows 11 snap layouts, a deferred
//! first show so the user never sees an unpainted window, hiding to the tray, and a
//! webview that stops drawing whenever the window is out of sight.

use mixtape_session::Session;
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
    // The window-state plugin has restored the bounds by now.
    fit_work_area(&window);
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
pub fn app_ready(window: tauri::WebviewWindow, state: tauri::State<'_, std::sync::Arc<Session>>) -> Result<(), String> {
    if IN_TRAY.load(Ordering::SeqCst) {
        tracing::info!(elapsed_ms = state.started.elapsed().as_millis() as u64, "ready in the tray");
        return Ok(());
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    tracing::info!(elapsed_ms = state.started.elapsed().as_millis() as u64, "window shown");
    Ok(())
}

/// A rectangle in physical pixels: x, y, width, height.
type Bounds = (i32, i32, i32, i32);

/// `rect` moved and, if it must, shrunk to lie inside `area`.
fn fit_into(rect: Bounds, area: Bounds) -> Bounds {
    let (ax, ay, aw, ah) = area;
    let w = rect.2.min(aw);
    let h = rect.3.min(ah);
    (rect.0.clamp(ax, ax + aw - w), rect.1.clamp(ay, ay + ah - h), w, h)
}

/// Keeps the window inside the work area of its screen. The restored bounds can reach
/// under the taskbar (saved while the taskbar was elsewhere or hidden, on another
/// screen layout, or while the window was snapped).
pub fn fit_work_area(w: &WebviewWindow) {
    if w.is_maximized().unwrap_or(false) || w.is_minimized().unwrap_or(false) {
        return;
    }
    let (Ok(pos), Ok(outer), Ok(inner), Ok(Some(monitor))) =
        (w.outer_position(), w.outer_size(), w.inner_size(), w.current_monitor())
    else {
        return;
    };
    let a = monitor.work_area();
    let area = (a.position.x, a.position.y, a.size.width as i32, a.size.height as i32);
    let rect = (pos.x, pos.y, outer.width as i32, outer.height as i32);
    let fitted = fit_into(rect, area);
    if fitted == rect {
        return;
    }
    tracing::info!(?rect, ?area, "window moved into the work area");
    if (fitted.2, fitted.3) != (rect.2, rect.3) {
        let frame = (outer.width as i32 - inner.width as i32, outer.height as i32 - inner.height as i32);
        let _ = w.set_size(tauri::PhysicalSize::new((fitted.2 - frame.0).max(1) as u32, (fitted.3 - frame.1).max(1) as u32));
    }
    let _ = w.set_position(tauri::PhysicalPosition::new(fitted.0, fitted.1));
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

/// The window is on the screen: not in the tray and not minimized (it may be behind
/// other windows).
pub fn on_screen(app: &AppHandle) -> bool {
    app.get_webview_window(MAIN).is_some_and(|w| {
        !IN_TRAY.load(Ordering::SeqCst) && w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false)
    })
}

/// Tray icon click: hides the window if it is the one in front, shows it otherwise.
pub fn toggle_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else { return };
    let blurred_just_now =
        BLURRED_AT.lock().expect("blur time poisoned").is_some_and(|t| t.elapsed() < BLUR_GRACE);
    if on_screen(app) && (w.is_focused().unwrap_or(false) || blurred_just_now) {
        hide_main(app);
    } else {
        show_main(app);
    }
}

/// The window's close button: to the tray, or out (the setting «close to tray»).
/// Returns whether the close was turned into hiding.
pub fn close_to_tray(app: &AppHandle) -> bool {
    let Some(st) = app.try_state::<std::sync::Arc<Session>>() else { return false };
    if !mixtape_session::commands::settings::Prefs::load(&st).close_to_tray {
        return false;
    }
    hide_main(app);
    true
}

/// Tray menu: hides the window when it is on the screen, shows it otherwise.
pub fn show_or_hide_main(app: &AppHandle) {
    if on_screen(app) { hide_main(app) } else { show_main(app) }
}

/// Quits the app (tray menu): saves the queue position, then exits.
pub fn quit(app: &AppHandle) {
    if let Some(st) = app.try_state::<std::sync::Arc<Session>>() {
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

#[cfg(test)]
mod tests {
    use super::fit_into;

    const AREA: (i32, i32, i32, i32) = (0, 0, 1920, 1032);

    #[test]
    fn a_window_inside_stays() {
        assert_eq!(fit_into((100, 100, 1280, 800), AREA), (100, 100, 1280, 800));
    }

    #[test]
    fn a_window_under_the_taskbar_moves_up_and_shrinks() {
        // Right half restored with the full screen height: the bottom is under the taskbar.
        assert_eq!(fit_into((960, 0, 960, 1080), AREA), (960, 0, 960, 1032));
        assert_eq!(fit_into((300, 400, 1280, 800), AREA), (300, 232, 1280, 800));
        // Off the left and top edges, and on a second screen.
        assert_eq!(fit_into((-50, -20, 800, 600), AREA), (0, 0, 800, 600));
        assert_eq!(fit_into((3000, 1200, 800, 600), (1920, 0, 2560, 1400)), (3000, 800, 800, 600));
    }
}
