//! Windows media integration: SMTC (media overlay and keys) and taskbar thumbnail
//! buttons, driven by player state.

use crate::state::Session;
use mixtape_core::Track;
use mixtape_player::{PlaybackState, Status};
use mixtape_win::{MediaCommand, Smtc, ThumbBar};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// SMTC does not advance the position itself; refresh it this often while playing. The
/// player sends state on every change and about every 5 s while playing (its drift
/// correction), so this takes each of those periodic updates.
const TIMELINE_EVERY: Duration = Duration::from_secs(4);

#[derive(Default)]
pub struct Media {
    smtc: OnceLock<Smtc>,
    thumbbar: OnceLock<ThumbBar>,
    /// 0 = unknown, 1 = paused, 2 = playing.
    playing: AtomicU8,
    last_timeline: Mutex<Option<Instant>>,
}

pub fn dispatch(app: &AppHandle, cmd: MediaCommand) {
    let Some(st) = app.try_state::<Arc<Session>>() else { return };
    let st = st.inner().clone();
    // Commands may arrive on the UI thread (thumbbar); never block it. A media key may
    // start the engine (resume after it was let go).
    std::thread::spawn(move || {
        let Ok(p) = st.player() else { return };
        match cmd {
            MediaCommand::Play => p.play(),
            MediaCommand::Pause | MediaCommand::Stop => p.pause(),
            MediaCommand::Toggle => p.toggle(),
            MediaCommand::Next => p.next(),
            MediaCommand::Previous => p.prev(),
            MediaCommand::Seek { position_ms } => p.seek(position_ms),
        }
    });
}

impl Media {
    /// Hooks up SMTC and (when the host has them) the thumbnail toolbar to the host's
    /// window. Runs on the UI thread.
    pub fn install(&self, app: &AppHandle) {
        let Some(host) = crate::host::host(app) else { return };
        let Some(hwnd) = host.smtc_hwnd(app) else { return };
        let a = app.clone();
        match Smtc::new(hwnd, move |c| dispatch(&a, c)) {
            Ok(s) => {
                let _ = self.smtc.set(s);
            }
            Err(e) => tracing::warn!(error = %e, "SMTC unavailable"),
        }
        if !host.taskbar_buttons() {
            return;
        }
        let a = app.clone();
        match ThumbBar::install(hwnd, move |c| dispatch(&a, c)) {
            Ok(t) => {
                let _ = self.thumbbar.set(t);
            }
            Err(e) => tracing::warn!(error = %e, "thumbnail toolbar unavailable"),
        }
    }

    pub fn on_state(&self, s: &PlaybackState) {
        let playing = matches!(s.status, Status::Playing | Status::Buffering | Status::Loading);
        let code = if playing { 2 } else { 1 };
        let changed = self.playing.swap(code, Ordering::Relaxed) != code;
        if changed {
            if let Some(smtc) = self.smtc.get() {
                let _ = smtc.set_playback(playing);
            }
            if let Some(tb) = self.thumbbar.get() {
                let _ = tb.set_playing(playing);
            }
        }
        let mut last = self.last_timeline.lock().expect("timeline lock poisoned");
        let due = last.is_none_or(|t| t.elapsed() >= TIMELINE_EVERY);
        if (changed || due) && s.duration_ms > 0 {
            if let Some(smtc) = self.smtc.get() {
                let _ = smtc.set_timeline(s.position_ms, s.duration_ms);
            }
            *last = Some(Instant::now());
        }
    }

    /// Forces a timeline refresh after seeks.
    pub fn timeline_dirty(&self) {
        *self.last_timeline.lock().expect("timeline lock poisoned") = None;
    }

    pub fn on_track(&self, st: &Session, track: &Track) {
        let Some(smtc) = self.smtc.get() else { return };
        let cover = track
            .cover_key
            .as_deref()
            .and_then(|k| st.covers.path(k, 300).ok())
            .filter(|p| p.exists());
        let album = track.album.as_ref().map(|a| a.title.as_str()).unwrap_or("");
        let _ = smtc.set_metadata(&track.title, &track.artist_names(), album, cover.as_deref());
        self.timeline_dirty();
    }
}
