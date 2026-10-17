//! Rare global events sent to the UI with `emit` (contracts/ipc.md, «События»), and
//! the bridge from player events to the UI and to Windows media controls.

use crate::state::AppState;
use mixtape_core::Track;
use mixtape_player::PlayerEvent;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use ts_rs::TS;

/// `track:changed` — a new track became current.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TrackChanged {
    pub track: Track,
    /// Cover colours, most prominent first; empty when unknown.
    pub palette: Vec<String>,
}

#[derive(Clone, Serialize)]
struct Toast {
    level: &'static str,
    text: String,
}

pub fn toast_error(app: &AppHandle, text: impl Into<String>) {
    let _ = app.emit("toast", Toast { level: "error", text: text.into() });
}

pub fn toast_info(app: &AppHandle, text: impl Into<String>) {
    let _ = app.emit("toast", Toast { level: "info", text: text.into() });
}

/// «1 трек», «3 трека», «12 треков».
pub fn tracks_word(n: usize) -> String {
    let (m10, m100) = (n % 10, n % 100);
    let word = if m10 == 1 && m100 != 11 {
        "трек"
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        "трека"
    } else {
        "треков"
    };
    format!("{n} {word}")
}

pub fn announce_track(st: &AppState, track: Track) {
    let palette = track
        .album
        .as_ref()
        .and_then(|a| st.db.read(|c| mixtape_library::repo::album(c, &a.id)).ok().flatten())
        .map(|a| a.palette)
        .unwrap_or_default();
    let _ = st.app.emit("track:changed", TrackChanged { track, palette });
}

/// Forwards player events. Called on the player's event thread.
pub fn on_player_event(app: &AppHandle, ev: PlayerEvent) {
    let Some(st) = app.try_state::<std::sync::Arc<AppState>>() else { return };
    match ev {
        PlayerEvent::State(s) => {
            if let Some(ch) = st.player_channel.lock().expect("channel slot poisoned").as_ref() {
                let _ = ch.send(s.clone());
            }
            st.media.on_state(&s);
        }
        PlayerEvent::TrackChanged { track, palette } => {
            st.media.on_track(&st, &track);
            let _ = app.emit("track:changed", TrackChanged { track, palette });
        }
        PlayerEvent::QueueChanged => {
            let _ = app.emit("queue:changed", ());
            crate::commands::mix::top_up(st.inner());
        }
        PlayerEvent::Played { track_id, full, full_plays } => {
            if full {
                crate::commands::mix::autosave(st.inner(), &track_id, full_plays);
            }
        }
        PlayerEvent::Error(msg) => toast_error(app, msg),
    }
}
