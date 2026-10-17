//! The tray icon's menu: a small frameless, transparent webview (`ui/tray-menu.html`) in
//! the app's own style instead of the native Win32 menu. It is created on the first right
//! click, kept hidden between uses and closed after two minutes unused, which gives its
//! memory back.
//!
//! The page drives showing and hiding: on `tray-menu:open` it asks for a snapshot,
//! measures its card and calls `tray_menu_reveal`, which places the window at the cursor
//! and brings it forward. On blur, Esc or an action it clears its content, waits for that
//! empty frame to be drawn and calls `tray_menu_hide`, so the next show never flashes the
//! previous content. While it is open it hears `track:changed` and `tray-menu:playing`.

use crate::commands::{CmdResult, blocking};
use crate::state::AppState;
use mixtape_core::{Lang, Track};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::async_runtime::JoinHandle;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent, webview::PageLoadEvent,
};
use ts_rs::TS;

pub const LABEL: &str = "tray-menu";
/// Unused this long, the window is closed.
const IDLE: Duration = Duration::from_secs(120);
/// Transparent margin around the card that its shadow draws into, logical px. Must match
/// `--pad` in `ui/src/tray-menu.css`.
const PAD: f64 = 16.0;
/// Least gap between the card and the work-area edges (the taskbar), logical px.
const EDGE: f64 = 8.0;
/// Card size until the page reports its own, logical px.
const DEFAULT_CARD: (f64, f64) = (264.0, 246.0);
/// Hidden by a blur this recently: a right click on the icon that caused the blur closes
/// the menu instead of opening it again.
const REOPEN_GRACE: Duration = Duration::from_millis(250);
/// The page hides itself on blur; if it does not answer in time, hide it from here.
const BLUR_FALLBACK: Duration = Duration::from_millis(300);
/// tauri-plugin-frame's native snap-layouts overlay (see `mini.rs`): the plugin installs it
/// on every page, and over this window it would swallow clicks at the top right.
const SNAP_OVERLAY_CLASS: &str = "TauriFrameSnapOverlay";

/// The window is on the screen.
static VISIBLE: AtomicBool = AtomicBool::new(false);
/// The window is being built (the first right click).
static CREATING: AtomicBool = AtomicBool::new(false);
/// Where the last right click was, physical px.
static ANCHOR: Mutex<Option<(f64, f64)>> = Mutex::new(None);
static HIDDEN_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// The one pending close-when-idle timer.
static IDLE_TIMER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// What the menu shows, read each time it opens.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TrayMenuSnapshot {
    pub lang: Lang,
    pub track: Option<Track>,
    pub playing: bool,
    /// The main window is on the screen: the first item hides it rather than shows it.
    pub window_shown: bool,
    pub mini: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TrayAction {
    /// Show the main window, or hide it when it is on the screen.
    Window,
    Mini,
    Settings,
    Quit,
}

/// Work area of a monitor, physical px.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Area {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// Window bounds (physical: x, y, width, height) for a card of `card` logical px whose
/// bottom-right corner sits at the cursor, kept `EDGE` inside the work area, so over a
/// taskbar it opens just above it like a native tray menu. The window is the card plus
/// `PAD` on every side.
fn place(cursor: (f64, f64), card: (f64, f64), scale: f64, area: Area) -> (i32, i32, u32, u32) {
    let (cw, ch) = ((card.0 * scale).round(), (card.1 * scale).round());
    let pad = (PAD * scale).round();
    let edge = (EDGE * scale).round();
    let clamp = |v: f64, lo: f64, hi: f64| v.min(hi).max(lo);
    let x = clamp(cursor.0 - cw, area.x + edge, area.x + area.w - edge - cw);
    let y = clamp(cursor.1 - ch, area.y + edge, area.y + area.h - edge - ch);
    (
        (x - pad) as i32,
        (y - pad) as i32,
        (cw + 2.0 * pad) as u32,
        (ch + 2.0 * pad) as u32,
    )
}

/// Right click on the tray icon. Main thread.
pub fn open(app: &AppHandle, cursor: PhysicalPosition<f64>) {
    let hidden_by_this_click = HIDDEN_AT
        .lock()
        .expect("tray menu time poisoned")
        .is_some_and(|t| t.elapsed() < REOPEN_GRACE);
    if VISIBLE.load(Ordering::SeqCst) || hidden_by_this_click {
        return;
    }
    *ANCHOR.lock().expect("tray menu anchor poisoned") = Some((cursor.x, cursor.y));
    cancel_idle();
    if app.get_webview_window(LABEL).is_some() {
        let _ = app.emit_to(LABEL, "tray-menu:open", ());
    } else if !CREATING.swap(true, Ordering::SeqCst) {
        // Building a webview waits on the event loop, so never on the main thread.
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = create(&app) {
                tracing::warn!(error = %e, "tray menu unavailable");
            }
            CREATING.store(false, Ordering::SeqCst);
        });
    }
}

/// Builds the hidden window; its page reveals it once it has drawn.
fn create(app: &AppHandle) -> tauri::Result<()> {
    let (w, h) = (DEFAULT_CARD.0 + 2.0 * PAD, DEFAULT_CARD.1 + 2.0 * PAD);
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("tray-menu.html".into()))
        .title("Mixtape")
        .inner_size(w, h)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .visible(false)
        .on_page_load(|w, p| {
            if p.event() == PageLoadEvent::Finished {
                let w2 = w.clone();
                let _ = w.run_on_main_thread(move || hide_snap_overlay(&w2));
            }
        })
        .build()?;
    let a = app.clone();
    window.on_window_event(move |e| {
        if let WindowEvent::Focused(false) = e {
            blur_fallback(&a);
        }
    });
    Ok(())
}

/// The frame plugin's overlay comes back on every resize; hide it after each.
fn hide_snap_overlay(w: &WebviewWindow) {
    if let Ok(hwnd) = w.hwnd() {
        mixtape_win::show_child_windows(hwnd.0 as isize, SNAP_OVERLAY_CLASS, false);
    }
}

fn blur_fallback(app: &AppHandle) {
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(BLUR_FALLBACK).await;
        let Some(w) = a.get_webview_window(LABEL) else {
            return;
        };
        if VISIBLE.load(Ordering::SeqCst) && !w.is_focused().unwrap_or(false) {
            hide(&a);
        }
    });
}

fn hide(app: &AppHandle) {
    let Some(w) = app.get_webview_window(LABEL) else {
        return;
    };
    if VISIBLE.swap(false, Ordering::SeqCst) {
        *HIDDEN_AT.lock().expect("tray menu time poisoned") = Some(Instant::now());
    }
    let _ = w.hide();
    arm_idle(app);
}

/// One timer at a time: closes the hidden window after `IDLE`.
fn arm_idle(app: &AppHandle) {
    let a = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        tokio::time::sleep(IDLE).await;
        if VISIBLE.load(Ordering::SeqCst) {
            return;
        }
        if let Some(w) = a.get_webview_window(LABEL)
            && let Ok(hwnd) = w.hwnd()
        {
            tracing::debug!("tray menu closed after idling");
            // A real WM_CLOSE: the frame plugin removes its overlay and subclass on it.
            mixtape_win::post_close(hwnd.0 as isize);
        }
    });
    if let Some(old) = IDLE_TIMER
        .lock()
        .expect("tray menu timer poisoned")
        .replace(task)
    {
        old.abort();
    }
}

fn cancel_idle() {
    if let Some(t) = IDLE_TIMER.lock().expect("tray menu timer poisoned").take() {
        t.abort();
    }
}

/// The player started or stopped: the open menu swaps Play and Pause.
pub fn on_playing(app: &AppHandle, playing: bool) {
    if VISIBLE.load(Ordering::SeqCst) {
        let _ = app.emit_to(LABEL, "tray-menu:playing", playing);
    }
}

#[tauri::command]
pub async fn tray_menu_snapshot(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> CmdResult<TrayMenuSnapshot> {
    let st = state.inner().clone();
    let track = blocking(move || Ok(st.player.current_track())).await?;
    Ok(TrayMenuSnapshot {
        lang: mixtape_core::l10n::lang(),
        track,
        playing: crate::tray::is_playing(&state.player.state()),
        window_shown: crate::window::on_screen(&app),
        mini: crate::mini::is_active(),
    })
}

/// The page has drawn its content: place the window at the last right click, show it and
/// take the focus, so a click anywhere else blurs it. `width`/`height` = the card, CSS px.
#[tauri::command]
pub fn tray_menu_reveal(window: WebviewWindow, width: f64, height: f64) {
    if window.label() != LABEL {
        return;
    }
    let Some(cursor) = *ANCHOR.lock().expect("tray menu anchor poisoned") else {
        return;
    };
    cancel_idle();
    let monitor = window
        .monitor_from_point(cursor.0, cursor.1)
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    let (scale, area) = monitor.map_or(
        (
            1.0,
            Area {
                x: 0.0,
                y: 0.0,
                w: cursor.0 + EDGE,
                h: cursor.1 + EDGE,
            },
        ),
        |m| {
            let a = m.work_area();
            (
                m.scale_factor(),
                Area {
                    x: a.position.x as f64,
                    y: a.position.y as f64,
                    w: a.size.width as f64,
                    h: a.size.height as f64,
                },
            )
        },
    );
    let card = if width > 0.0 && height > 0.0 {
        (width, height)
    } else {
        DEFAULT_CARD
    };
    let (x, y, w, h) = place(cursor, card, scale, area);
    // Position first: moving onto a screen with another scale resizes the window.
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_size(PhysicalSize::new(w, h));
    let _ = window.show();
    let _ = window.set_focus();
    VISIBLE.store(true, Ordering::SeqCst);
    hide_snap_overlay(&window);
    // Again after the frame plugin has handled the resize (it shows the overlay then).
    let w2 = window.clone();
    let _ = window.run_on_main_thread(move || hide_snap_overlay(&w2));
}

/// The page has cleared its content (blur, Esc, an action).
#[tauri::command]
pub fn tray_menu_hide(app: AppHandle) {
    hide(&app);
}

/// A menu item. Main thread (a sync command), as the window calls need.
#[tauri::command]
pub fn tray_menu_action(app: AppHandle, action: TrayAction) {
    match action {
        TrayAction::Window => crate::window::show_or_hide_main(&app),
        TrayAction::Mini => crate::mini::toggle(&app),
        TrayAction::Settings => {
            // Settings need the full window.
            if crate::mini::is_active() {
                crate::mini::toggle(&app);
            }
            crate::window::show_main(&app);
            let _ = app.emit_to(crate::window::MAIN, "app:open-settings", ());
        }
        TrayAction::Quit => crate::window::quit(&app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1920×1080 with a 48 px taskbar at the bottom.
    const SCREEN: Area = Area {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1032.0,
    };

    #[test]
    fn opens_above_the_taskbar_left_of_the_cursor() {
        // A tray icon near the clock: the card ends at the cursor horizontally and sits
        // EDGE above the taskbar.
        let (x, y, w, h) = place((1700.0, 1056.0), (264.0, 300.0), 1.0, SCREEN);
        assert_eq!((w, h), (264 + 32, 300 + 32));
        assert_eq!(x + PAD as i32, 1700 - 264);
        assert_eq!(y + PAD as i32 + 300, 1032 - EDGE as i32);
    }

    #[test]
    fn stays_inside_the_work_area() {
        // Right at the screen's right edge: the card keeps EDGE from it.
        let (x, _, _, _) = place((1915.0, 1056.0), (264.0, 300.0), 1.0, SCREEN);
        assert_eq!(x + PAD as i32, 1920 - EDGE as i32 - 264);
        // Cursor at the far left: the card is pushed right.
        let (x, _, _, _) = place((100.0, 1056.0), (264.0, 300.0), 1.0, SCREEN);
        assert_eq!(x + PAD as i32, EDGE as i32);
        // Taskbar at the top: the card hangs from the work area's top edge.
        let top = Area { y: 48.0, ..SCREEN };
        let (_, y, _, _) = place((1700.0, 20.0), (264.0, 300.0), 1.0, top);
        assert_eq!(y + PAD as i32, 48 + EDGE as i32);
    }

    #[test]
    fn scales_to_physical_pixels() {
        let second = Area {
            x: 1920.0,
            y: 0.0,
            w: 2560.0,
            h: 1380.0,
        };
        let (x, y, w, h) = place((4300.0, 1420.0), (264.0, 300.0), 1.5, second);
        assert_eq!((w, h), (396 + 48, 450 + 48));
        assert_eq!(x + 24, 4300 - 396);
        assert_eq!(y + 24 + 450, 1380 - 12);
    }
}
