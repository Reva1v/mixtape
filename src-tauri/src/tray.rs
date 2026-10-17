//! Notification-area icon: a left click shows or hides the window, a right click opens
//! the menu (`tray_menu`, a small webview in the app's style). The tooltip follows the
//! player's events; nothing polls.

use crate::state::AppState;
use mixtape_core::Track;
use mixtape_player::{PlaybackState, Status};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

/// Windows cuts tray tooltips at 128 UTF-16 units, terminator included.
const TOOLTIP_MAX: usize = 127;

pub struct Tray {
    icon: TrayIcon,
    playing: AtomicBool,
    /// «Title — Artist» of the current track.
    track: Mutex<Option<String>>,
}

/// Playing for the tray: loading and buffering count, so Pause does not flicker to Play
/// between tracks.
pub fn is_playing(s: &PlaybackState) -> bool {
    matches!(
        s.status,
        Status::Playing | Status::Buffering | Status::Loading
    )
}

/// Builds the icon. Runs once, from `setup`.
pub fn install(app: &AppHandle, st: &AppState) -> tauri::Result<()> {
    let mut builder = TrayIconBuilder::with_id("main")
        .show_menu_on_left_click(false)
        .tooltip("Mixtape")
        .on_tray_icon_event(|tray, e| {
            let TrayIconEvent::Click {
                button,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = e
            else {
                return;
            };
            match button {
                MouseButton::Left => crate::window::toggle_main(tray.app_handle()),
                MouseButton::Right => crate::tray_menu::open(tray.app_handle(), position),
                MouseButton::Middle => {}
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let icon = builder.build(app)?;
    let tray = Tray {
        icon,
        playing: AtomicBool::new(is_playing(&st.player.state())),
        track: Mutex::new(None),
    };
    tray.retooltip();
    app.manage(tray);
    Ok(())
}

impl Tray {
    /// The tooltip names the track while it plays. Main thread only.
    fn retooltip(&self) {
        let playing = self.playing.load(Ordering::Relaxed);
        let track = self.track.lock().expect("tray track poisoned").clone();
        let _ = self
            .icon
            .set_tooltip(Some(tooltip(track.as_deref().filter(|_| playing))));
    }
}

/// Runs `f` on the tray from the main thread, without waiting for it: tray calls made on
/// other threads would block until the main thread gets to them.
fn with_tray(app: &AppHandle, f: impl FnOnce(&Tray) + Send + 'static) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(t) = a.try_state::<Tray>() {
            f(&t);
        }
    });
}

/// Player state changed: the tooltip and the open menu follow the status only.
pub fn on_state(app: &AppHandle, s: &PlaybackState) {
    let playing = is_playing(s);
    let Some(t) = app.try_state::<Tray>() else {
        return;
    };
    if t.playing.swap(playing, Ordering::Relaxed) != playing {
        with_tray(app, Tray::retooltip);
        crate::tray_menu::on_playing(app, playing);
    }
}

pub fn on_track(app: &AppHandle, track: &Track) {
    let Some(t) = app.try_state::<Tray>() else {
        return;
    };
    *t.track.lock().expect("tray track poisoned") =
        Some(format!("{} — {}", track.title, track.artist_names()));
    with_tray(app, Tray::retooltip);
}

/// «Mixtape», plus the playing track on a second line, cut to what Windows shows.
fn tooltip(track: Option<&str>) -> String {
    let Some(track) = track else {
        return "Mixtape".into();
    };
    let mut text = format!("Mixtape\n{track}");
    if text.encode_utf16().count() > TOOLTIP_MAX {
        while text.encode_utf16().count() > TOOLTIP_MAX - 1 {
            text.pop();
        }
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_fits_windows_limit() {
        assert_eq!(tooltip(None), "Mixtape");
        assert_eq!(
            tooltip(Some("Take On Me — a-ha")),
            "Mixtape\nTake On Me — a-ha"
        );
        let long = "Довга назва ".repeat(20);
        let tip = tooltip(Some(&long));
        assert_eq!(tip.encode_utf16().count(), TOOLTIP_MAX);
        assert!(tip.starts_with("Mixtape\nДовга") && tip.ends_with('…'));
    }
}
