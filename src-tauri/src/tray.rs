//! Notification-area icon: a left click shows or hides the window; the menu has the
//! transport controls, the mini player and Quit. Labels and the tooltip follow the
//! language and the player's events; nothing polls.

use crate::state::AppState;
use mixtape_core::{Track, tr};
use mixtape_player::{PlaybackState, Status};
use mixtape_win::MediaCommand;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

/// Windows cuts tray tooltips at 128 UTF-16 units, terminator included.
const TOOLTIP_MAX: usize = 127;

pub struct Tray {
    icon: TrayIcon,
    show: MenuItem<tauri::Wry>,
    toggle: MenuItem<tauri::Wry>,
    next: MenuItem<tauri::Wry>,
    prev: MenuItem<tauri::Wry>,
    mini: CheckMenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
    playing: AtomicBool,
    /// «Title — Artist» of the current track.
    track: Mutex<Option<String>>,
}

fn is_playing(s: &PlaybackState) -> bool {
    matches!(
        s.status,
        Status::Playing | Status::Buffering | Status::Loading
    )
}

/// Builds the icon and its menu. Runs once, from `setup`.
pub fn install(app: &AppHandle, st: &AppState) -> tauri::Result<()> {
    let item = |id: &str| MenuItem::with_id(app, id, id, true, None::<&str>);
    let (show, toggle, next, prev, quit) = (
        item("show")?,
        item("toggle")?,
        item("next")?,
        item("prev")?,
        item("quit")?,
    );
    let mini = CheckMenuItem::with_id(app, "mini", "mini", true, false, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &sep()?,
            &toggle,
            &next,
            &prev,
            &sep()?,
            &mini,
            &sep()?,
            &quit,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Mixtape")
        .on_menu_event(|app, e| on_menu(app, e.id().as_ref()))
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                crate::window::toggle_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let icon = builder.build(app)?;
    let tray = Tray {
        icon,
        show,
        toggle,
        next,
        prev,
        mini,
        quit,
        playing: AtomicBool::new(is_playing(&st.player.state())),
        track: Mutex::new(None),
    };
    tray.relabel();
    app.manage(tray);
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "show" => crate::window::show_main(app),
        "toggle" => crate::media::dispatch(app, MediaCommand::Toggle),
        "next" => crate::media::dispatch(app, MediaCommand::Next),
        "prev" => crate::media::dispatch(app, MediaCommand::Previous),
        "mini" => crate::mini::toggle(app),
        "quit" => crate::window::quit(app),
        _ => {}
    }
}

impl Tray {
    /// Every label in the current language, and the tooltip. Main thread only.
    fn relabel(&self) {
        let _ = self
            .show
            .set_text(tr("Show Mixtape", "Показати Mixtape", "Показать Mixtape"));
        let _ = self.next.set_text(tr("Next", "Наступний", "Следующий"));
        let _ = self
            .prev
            .set_text(tr("Previous", "Попередній", "Предыдущий"));
        let _ = self
            .mini
            .set_text(tr("Mini player", "Мініплеєр", "Мини-плеер"));
        let _ = self.quit.set_text(tr("Quit", "Вийти", "Выйти"));
        self.relabel_playing();
    }

    fn relabel_playing(&self) {
        let playing = self.playing.load(Ordering::Relaxed);
        let label = if playing {
            tr("Pause", "Пауза", "Пауза")
        } else {
            tr("Play", "Відтворити", "Воспроизвести")
        };
        let _ = self.toggle.set_text(label);
        let track = self.track.lock().expect("tray track poisoned").clone();
        let _ = self
            .icon
            .set_tooltip(Some(tooltip(track.as_deref().filter(|_| playing))));
    }
}

/// Runs `f` on the tray from the main thread, without waiting for it: menu calls made on
/// other threads would block until the main thread gets to them.
fn with_tray(app: &AppHandle, f: impl FnOnce(&Tray) + Send + 'static) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(t) = a.try_state::<Tray>() {
            f(&t);
        }
    });
}

/// Player state changed: the Play/Pause label and the tooltip follow the status only.
pub fn on_state(app: &AppHandle, s: &PlaybackState) {
    let playing = is_playing(s);
    let Some(t) = app.try_state::<Tray>() else {
        return;
    };
    if t.playing.swap(playing, Ordering::Relaxed) != playing {
        with_tray(app, Tray::relabel_playing);
    }
}

pub fn on_track(app: &AppHandle, track: &Track) {
    let Some(t) = app.try_state::<Tray>() else {
        return;
    };
    *t.track.lock().expect("tray track poisoned") =
        Some(format!("{} — {}", track.title, track.artist_names()));
    with_tray(app, Tray::relabel_playing);
}

/// The interface language changed.
pub fn refresh(app: &AppHandle) {
    with_tray(app, Tray::relabel);
}

pub fn set_mini(app: &AppHandle, on: bool) {
    with_tray(app, move |t| {
        let _ = t.mini.set_checked(on);
    });
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
