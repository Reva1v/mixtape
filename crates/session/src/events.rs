//! Rare global events sent to the UI with `emit` (contracts/ipc.md, «События»), and
//! the bridge from player events to the UI and to Windows media controls.

use crate::state::Session;
use mixtape_core::{Lang, Track};
use mixtape_player::PlayerEvent;
use serde::Serialize;
use tauri::{AppHandle, Manager};
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
    crate::host::emit(app, "toast", Toast { level: "error", text: text.into() });
}

pub fn toast_info(app: &AppHandle, text: impl Into<String>) {
    crate::host::emit(app, "toast", Toast { level: "info", text: text.into() });
}

/// "3 tracks" in the app language.
pub fn tracks_word(n: usize) -> String {
    tracks_word_in(mixtape_core::l10n::lang(), n)
}

/// "1 track", "3 tracks"; «1 трек», «3 треки», «12 треків»; «1 трек», «3 трека», «12 треков».
fn tracks_word_in(lang: Lang, n: usize) -> String {
    let (m10, m100) = (n % 10, n % 100);
    let one = m10 == 1 && m100 != 11;
    let few = (2..=4).contains(&m10) && !(12..=14).contains(&m100);
    let word = match lang {
        Lang::En if n == 1 => "track",
        Lang::En => "tracks",
        Lang::Uk if one => "трек",
        Lang::Uk if few => "треки",
        Lang::Uk => "треків",
        Lang::Ru if one => "трек",
        Lang::Ru if few => "трека",
        Lang::Ru => "треков",
    };
    format!("{n} {word}")
}

pub fn announce_track(st: &Session, track: Track) {
    let palette = track
        .album
        .as_ref()
        .and_then(|a| st.db.read(|c| mixtape_library::repo::album(c, &a.id)).ok().flatten())
        .map(|a| a.palette)
        .unwrap_or_default();
    crate::host::emit(&st.app, "track:changed", TrackChanged { track, palette });
}

/// Forwards player events. Called on the player's event thread.
pub fn on_player_event(app: &AppHandle, ev: PlayerEvent) {
    let Some(st) = app.try_state::<std::sync::Arc<Session>>() else { return };
    crate::lastfm::on_player_event(st.inner(), &ev);
    crate::discord::on_player_event(&st, &ev);
    match ev {
        PlayerEvent::State(s) => {
            if let Some(ch) = st.player_channel.lock().expect("channel slot poisoned").as_ref() {
                let _ = ch.send(s.clone());
            }
            st.media.on_state(&s);
            st.host.on_state(app, &s);
        }
        PlayerEvent::TrackChanged { track, palette } => {
            // The analyser looks at the new current and next track (only a message: event
            // handlers must not call into the player).
            st.analyzer.kick();
            st.media.on_track(&st, &track);
            st.host.on_track(app, &track);
            crate::host::emit(app, "track:changed", TrackChanged { track, palette });
        }
        PlayerEvent::QueueChanged => {
            crate::host::emit(app, "queue:changed", ());
            crate::commands::mix::top_up(st.inner());
        }
        PlayerEvent::Played { track_id, full, full_plays } => {
            if full {
                crate::commands::mix::autosave(st.inner(), &track_id, full_plays);
            }
        }
        // The statistics screen re-reads its recap (debounced on its side).
        PlayerEvent::Listened(_) => {
            crate::host::emit(app, "stats:changed", ());
        }
        PlayerEvent::Error(msg) => toast_error(app, msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_word_plurals() {
        let words = |lang| [1, 2, 5, 11, 21, 22, 112].map(|n| tracks_word_in(lang, n));
        assert_eq!(
            words(Lang::En),
            ["1 track", "2 tracks", "5 tracks", "11 tracks", "21 tracks", "22 tracks", "112 tracks"]
        );
        assert_eq!(
            words(Lang::Uk),
            ["1 трек", "2 треки", "5 треків", "11 треків", "21 трек", "22 треки", "112 треків"]
        );
        assert_eq!(
            words(Lang::Ru),
            ["1 трек", "2 трека", "5 треков", "11 треков", "21 трек", "22 трека", "112 треков"]
        );
    }
}
