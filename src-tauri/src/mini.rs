//! Mini player: the main window itself turns into a small always-on-top strip (a second
//! webview would cost another renderer process). The UI swaps its layout on
//! `window:mini`; this side remembers the normal bounds, sizes the window, keeps the
//! window-state plugin from taking the mini size for the normal one, and remembers where
//! the strip was.

use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
};

/// Mini window size, logical px; fixed, the strip is not resizable.
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 116.0;
/// `minWidth` / `minHeight` of the main window in `tauri.conf.json`.
const NORMAL_MIN: (f64, f64) = (940.0, 600.0);
/// Gap to the work-area corner when the strip has no remembered place, logical px.
const MARGIN: f64 = 16.0;
/// tauri-plugin-frame's native snap-layouts overlay: a child window over the maximize
/// button's spot, which would swallow clicks on the strip.
const SNAP_OVERLAY_CLASS: &str = "TauriFrameSnapOverlay";
/// Settings key of the strip's last place.
const SAVED_KEY: &str = "mini_window";

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The normal window's bounds while the mini player is on.
static NORMAL: Mutex<Option<Normal>> = Mutex::new(None);

/// Physical pixels: outer position, inner size (what the window-state plugin stores).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

impl Rect {
    fn overlap(&self, o: &Rect) -> (i64, i64) {
        let w = (self.x as i64 + self.w as i64).min(o.x as i64 + o.w as i64)
            - (self.x as i64).max(o.x as i64);
        let h = (self.y as i64 + self.h as i64).min(o.y as i64 + o.h as i64)
            - (self.y as i64).max(o.y as i64);
        (w.max(0), h.max(0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Normal {
    bounds: Rect,
    maximized: bool,
}

/// Where the strip was last time: physical position. Older builds also stored a
/// `width`; serde skips that unknown field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    x: i32,
    y: i32,
}

/// Where the strip opens: its last place if at least half of it is still on one screen,
/// else the bottom-right corner of the work area the window is on.
fn place(
    saved: Option<(i32, i32)>,
    size: (u32, u32),
    areas: &[Rect],
    current: Rect,
    margin: i32,
) -> (i32, i32) {
    if let Some((x, y)) = saved {
        let r = Rect {
            x,
            y,
            w: size.0,
            h: size.1,
        };
        let visible = |a: &Rect| {
            let (w, h) = r.overlap(a);
            w * 2 >= size.0 as i64 && h * 2 >= size.1 as i64
        };
        if areas.iter().any(visible) {
            return (x, y);
        }
    }
    (
        current.x + current.w as i32 - size.0 as i32 - margin,
        current.y + current.h as i32 - size.1 as i32 - margin,
    )
}

/// Puts the normal bounds into the window-state plugin's saved JSON in place of the
/// mini ones, so the next start opens the full window where it was.
fn patch_window_state(json: &mut serde_json::Value, label: &str, normal: &Normal) -> bool {
    let Some(m) = json.get_mut(label).and_then(|v| v.as_object_mut()) else {
        return false;
    };
    let b = normal.bounds;
    for (k, v) in [("x", b.x), ("y", b.y), ("prev_x", b.x), ("prev_y", b.y)] {
        m.insert(k.into(), v.into());
    }
    m.insert("width".into(), b.w.into());
    m.insert("height".into(), b.h.into());
    m.insert("maximized".into(), normal.maximized.into());
    true
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(crate::window::MAIN)
}

fn overlay(w: &WebviewWindow, show: bool) {
    if let Ok(hwnd) = w.hwnd() {
        mixtape_win::show_child_windows(hwnd.0 as isize, SNAP_OVERLAY_CLASS, show);
    }
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// Window resized: a resize re-shows the snap overlay; keep it hidden while mini.
pub fn on_resized(w: &WebviewWindow) {
    if !is_active() {
        return;
    }
    let w2 = w.clone();
    // After the frame plugin's own WM_SIZE handling, which shows the overlay again.
    let _ = w.run_on_main_thread(move || {
        overlay(&w2, false);
        keep_size(&w2);
    });
}

/// Windows Snap ignores the size limits: dragging the strip onto a screen edge or a
/// snap layout zone stretches it to the zone. Put it back to the strip size.
fn keep_size(w: &WebviewWindow) {
    if w.is_minimized().unwrap_or(false) {
        return;
    }
    if w.is_maximized().unwrap_or(false) {
        let _ = w.unmaximize();
    }
    let (Ok(size), Ok(scale)) = (w.inner_size(), w.scale_factor()) else {
        return;
    };
    let want = ((WIDTH * scale).round() as i64, (HEIGHT * scale).round() as i64);
    if (size.width as i64 - want.0).abs() > 1 || (size.height as i64 - want.1).abs() > 1 {
        let _ = w.set_size(LogicalSize::new(WIDTH, HEIGHT));
    }
}

pub fn toggle(app: &AppHandle) {
    if is_active() { leave(app) } else { enter(app) }
}

fn load_saved(app: &AppHandle) -> Option<Saved> {
    let st = app.try_state::<Arc<AppState>>()?;
    mixtape_library::settings::get(&st.db, SAVED_KEY)
        .ok()
        .flatten()
}

/// Remembers the strip's place for next time.
fn remember(app: &AppHandle, w: &WebviewWindow) {
    if w.is_minimized().unwrap_or(false) {
        return;
    }
    let Ok(pos) = w.outer_position() else {
        return;
    };
    let saved = Saved { x: pos.x, y: pos.y };
    if let Some(st) = app.try_state::<Arc<AppState>>()
        && let Err(e) = mixtape_library::settings::set(&st.db, SAVED_KEY, &saved)
    {
        tracing::warn!(error = %e, "mini player place not saved");
    }
}

/// Main thread only (sync command, tray menu): window calls then apply at once.
fn enter(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::window::show_main(app);
    let maximized = w.is_maximized().unwrap_or(false);
    if maximized {
        let _ = w.unmaximize();
    }
    let (Ok(pos), Ok(size)) = (w.outer_position(), w.inner_size()) else {
        ACTIVE.store(false, Ordering::SeqCst);
        return;
    };
    *NORMAL.lock().expect("mini bounds poisoned") = Some(Normal {
        bounds: Rect {
            x: pos.x,
            y: pos.y,
            w: size.width,
            h: size.height,
        },
        maximized,
    });
    // The page swaps to the compact layout first: a frame of it in the big window looks
    // better than a frame of the full layout squeezed into the small one.
    let _ = app.emit("window:mini", true);

    let saved = load_saved(app);
    let scale = w.scale_factor().unwrap_or(1.0);
    let px = (
        (WIDTH * scale).round() as u32,
        (HEIGHT * scale).round() as u32,
    );
    let areas: Vec<Rect> = w
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let a = m.work_area();
            Rect {
                x: a.position.x,
                y: a.position.y,
                w: a.size.width,
                h: a.size.height,
            }
        })
        .collect();
    let current = w
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let a = m.work_area();
            Rect {
                x: a.position.x,
                y: a.position.y,
                w: a.size.width,
                h: a.size.height,
            }
        })
        .unwrap_or(Rect {
            x: pos.x,
            y: pos.y,
            w: size.width,
            h: size.height,
        });
    let (x, y) = place(
        saved.map(|s| (s.x, s.y)),
        px,
        &areas,
        current,
        (MARGIN * scale).round() as i32,
    );

    let _ = w.set_maximizable(false);
    let _ = w.set_always_on_top(true);
    let _ = w.set_min_size(Some(LogicalSize::new(WIDTH, HEIGHT)));
    let _ = w.set_max_size(Some(LogicalSize::new(WIDTH, HEIGHT)));
    let _ = w.set_size(LogicalSize::new(WIDTH, HEIGHT));
    let _ = w.set_resizable(false);
    let _ = w.set_position(PhysicalPosition::new(x, y));
    overlay(&w, false);
}

/// Main thread only, like [`enter`].
fn leave(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };
    if !ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    if w.is_minimized().unwrap_or(false) {
        let _ = w.unminimize();
    }
    remember(app, &w);
    ACTIVE.store(false, Ordering::SeqCst);
    let normal = NORMAL.lock().expect("mini bounds poisoned").take();
    let _ = w.set_always_on_top(false);
    // `tauri.conf.json` leaves the main window resizable (the default).
    let _ = w.set_resizable(true);
    let _ = w.set_max_size(None::<tauri::Size>);
    let _ = w.set_min_size(Some(LogicalSize::new(NORMAL_MIN.0, NORMAL_MIN.1)));
    if let Some(n) = normal {
        let _ = w.set_size(PhysicalSize::new(n.bounds.w, n.bounds.h));
        let _ = w.set_position(PhysicalPosition::new(n.bounds.x, n.bounds.y));
    }
    let _ = w.set_maximizable(true);
    overlay(&w, true);
    if normal.is_some_and(|n| n.maximized) {
        let _ = w.maximize();
    }
    let _ = app.emit("window:mini", false);
    crate::window::show_main(app);
}

/// The app is exiting (after the window-state plugin saved its file): in mini mode,
/// remember the strip and write the normal bounds over the mini ones in that file.
pub fn on_exit(app: &AppHandle) {
    if !is_active() {
        return;
    }
    if let Some(w) = main_window(app) {
        remember(app, &w);
    }
    let Some(normal) = *NORMAL.lock().expect("mini bounds poisoned") else {
        return;
    };
    use tauri_plugin_window_state::AppHandleExt;
    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    let path = dir.join(app.filename());
    let Ok(mut json) = std::fs::read(&path)
        .map_err(drop)
        .and_then(|b| serde_json::from_slice(&b).map_err(drop))
    else {
        return;
    };
    if patch_window_state(&mut json, crate::window::MAIN, &normal)
        && let Ok(bytes) = serde_json::to_vec_pretty(&json)
        && let Err(e) = std::fs::write(&path, bytes)
    {
        tracing::warn!(error = %e, "window state not corrected after mini player");
    }
}

/// Turns the mini player on or off (bottom bar button, Ctrl+M).
#[tauri::command]
pub fn window_set_mini(app: AppHandle, on: bool) {
    if on { enter(&app) } else { leave(&app) }
}

/// Whether the mini player is on (the page asks after a reload).
#[tauri::command]
pub fn window_is_mini() -> bool {
    is_active()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1040,
    };
    const SECOND: Rect = Rect {
        x: 1920,
        y: 0,
        w: 2560,
        h: 1400,
    };

    #[test]
    fn opens_where_it_was_while_that_is_on_screen() {
        let areas = [SCREEN, SECOND];
        assert_eq!(
            place(Some((100, 200)), (380, 116), &areas, SCREEN, 16),
            (100, 200)
        );
        assert_eq!(
            place(Some((3000, 900)), (380, 116), &areas, SCREEN, 16),
            (3000, 900)
        );
        // Half off the edge still counts; more than that does not.
        assert_eq!(
            place(Some((1730, 500)), (380, 116), &[SCREEN], SCREEN, 16),
            (1730, 500)
        );
        assert_eq!(
            place(Some((1800, 500)), (380, 116), &[SCREEN], SCREEN, 16),
            (1524, 908)
        );
    }

    #[test]
    fn defaults_to_the_work_area_corner() {
        assert_eq!(place(None, (380, 116), &[SCREEN], SCREEN, 16), (1524, 908));
        assert_eq!(
            place(None, (570, 174), &[SCREEN, SECOND], SECOND, 24),
            (1920 + 2560 - 570 - 24, 1400 - 174 - 24)
        );
        // The second screen was unplugged.
        assert_eq!(
            place(Some((3000, 900)), (380, 116), &[SCREEN], SCREEN, 16),
            (1524, 908)
        );
    }

    #[test]
    fn saved_place_reads_old_json_with_a_width() {
        let old: Saved = serde_json::from_str(r#"{"x":10,"y":-20,"width":560.0}"#).unwrap();
        assert_eq!(old, Saved { x: 10, y: -20 });
    }

    #[test]
    fn window_state_gets_the_normal_bounds() {
        let mut json = serde_json::json!({
            "main": { "width": 380, "height": 116, "x": 1500, "y": 900, "prev_x": 1400, "prev_y": 800,
                      "maximized": false, "visible": true, "decorated": false, "fullscreen": false },
            "login": { "width": 500 }
        });
        let normal = Normal {
            bounds: Rect {
                x: 80,
                y: 60,
                w: 1280,
                h: 800,
            },
            maximized: true,
        };
        assert!(patch_window_state(&mut json, "main", &normal));
        let m = &json["main"];
        assert_eq!((m["x"].as_i64(), m["y"].as_i64()), (Some(80), Some(60)));
        assert_eq!(
            (m["prev_x"].as_i64(), m["prev_y"].as_i64()),
            (Some(80), Some(60))
        );
        assert_eq!(
            (m["width"].as_u64(), m["height"].as_u64()),
            (Some(1280), Some(800))
        );
        assert_eq!(m["maximized"].as_bool(), Some(true));
        assert_eq!(m["decorated"].as_bool(), Some(false));
        assert_eq!(json["login"]["width"].as_u64(), Some(500));
        assert!(!patch_window_state(&mut json, "other", &normal));
    }
}
