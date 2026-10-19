//! Background analysis for smart transitions: one thread, at background priority, analyses
//! every track with an audio file once (`track_analysis`), the current and next track first.
//! It runs only while smart transitions are on, and when there is nothing left it sleeps until
//! [`Analyzer::kick`] says new audio may have arrived — no timers, no polling.

use mixtape_audio::{ANALYSIS_VERSION, Key, TrackAnalysis, analyze_file};
use mixtape_core::TrackId;
use mixtape_library::Db;
use mixtape_library::analysis::{self, StoredAnalysis};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::Player;

/// Breather between two tracks, so a large library is worked through gently.
const PAUSE: Duration = Duration::from_millis(300);

/// What the worker needs from the app.
pub struct AnalyzerHooks {
    /// Tracks to analyse before the rest (the current and the next one).
    pub upcoming: Box<dyn Fn() -> Vec<TrackId> + Send>,
    /// An analysis was stored.
    pub analysed: Box<dyn Fn(&TrackId) + Send>,
    /// Runs first on the worker thread (e.g. to lower its priority).
    pub on_start: Box<dyn FnOnce() + Send>,
}

impl AnalyzerHooks {
    /// Hooks for the app's player: its upcoming tracks first, results handed to it.
    pub fn for_player(player: &Arc<Player>, on_start: impl FnOnce() + Send + 'static) -> Self {
        let (p1, p2): (Weak<Player>, Weak<Player>) =
            (Arc::downgrade(player), Arc::downgrade(player));
        Self {
            upcoming: Box::new(move || p1.upgrade().map(|p| p.upcoming()).unwrap_or_default()),
            analysed: Box::new(move |id| {
                if let Some(p) = p2.upgrade() {
                    p.refresh_analysis(id);
                }
            }),
            on_start: Box::new(on_start),
        }
    }
}

enum Msg {
    Enable(bool),
    Kick,
}

/// Handle to the worker thread; it ends when the handle is dropped.
pub struct Analyzer {
    tx: Sender<Msg>,
}

impl Analyzer {
    /// Starts the worker, disabled. `root` is the data directory media paths are relative to.
    pub fn start(db: Db, root: PathBuf, hooks: AnalyzerHooks) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("track-analysis".into())
            .spawn(move || run(db, root, hooks, rx))?;
        Ok(Self { tx })
    }

    /// On while smart transitions are on.
    pub fn set_enabled(&self, on: bool) {
        let _ = self.tx.send(Msg::Enable(on));
    }

    /// New audio may be on disk, or the queue moved on: look for work again. Cheap; safe to
    /// call from event handlers (it only sends a message).
    pub fn kick(&self) {
        let _ = self.tx.send(Msg::Kick);
    }
}

fn run(db: Db, root: PathBuf, hooks: AnalyzerHooks, rx: Receiver<Msg>) {
    let AnalyzerHooks {
        upcoming,
        analysed,
        on_start,
    } = hooks;
    on_start();
    let mut enabled = false;
    let mut idle = true;
    loop {
        let msg = if !enabled || idle {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        } else {
            match rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };
        for m in msg
            .into_iter()
            .chain(std::iter::from_fn(|| rx.try_recv().ok()))
        {
            match m {
                Msg::Enable(on) => enabled = on,
                Msg::Kick => {}
            }
            idle = false;
        }
        if !enabled {
            continue;
        }
        let Some((id, rel)) = next_job(&db, &upcoming()) else {
            idle = true;
            continue;
        };
        let started = std::time::Instant::now();
        let result = analyze_file(&root.join(&rel));
        let a = match result {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!(%id, error = %e, "track analysis failed");
                TrackAnalysis::empty()
            }
        };
        tracing::debug!(%id, bpm = a.bpm, key = ?a.key, ms = started.elapsed().as_millis() as u64, "track analysed");
        let stored = to_stored(&a);
        let id2 = id.clone();
        match db.write(move |t| analysis::set(t, &id2, &stored)) {
            Ok(()) => analysed(&id),
            Err(e) => {
                // Keep the database from being hammered by a persistent error.
                tracing::warn!(%id, error = %e, "storing the track analysis failed");
                idle = true;
                continue;
            }
        }
        std::thread::sleep(PAUSE);
    }
}

/// The first of `upcoming` still to analyse, else any.
fn next_job(db: &Db, upcoming: &[TrackId]) -> Option<(TrackId, String)> {
    for id in upcoming {
        if let Ok(Some(path)) = db.read(|c| analysis::needs(c, id, ANALYSIS_VERSION)) {
            return Some((id.clone(), path));
        }
    }
    db.read(|c| analysis::pending(c, ANALYSIS_VERSION, 1))
        .ok()?
        .into_iter()
        .next()
}

pub(crate) fn to_stored(a: &TrackAnalysis) -> StoredAnalysis {
    StoredAnalysis {
        version: ANALYSIS_VERSION,
        bpm: a.bpm as f64,
        beat_ms: a.beat_ms as f64,
        key: a.key.map(Key::index),
        start_ms: a.start_ms,
        end_ms: a.end_ms,
        intro_end_ms: a.intro_end_ms,
        outro_start_ms: a.outro_start_ms,
    }
}

/// The audio crate's view of a stored analysis; `None` when it is from another version.
pub(crate) fn to_audio(s: &StoredAnalysis) -> Option<TrackAnalysis> {
    (s.version == ANALYSIS_VERSION).then(|| TrackAnalysis {
        bpm: s.bpm as f32,
        beat_ms: s.beat_ms as f32,
        key: s.key.and_then(Key::from_index),
        start_ms: s.start_ms,
        end_ms: s.end_ms,
        intro_end_ms: s.intro_end_ms,
        outro_start_ms: s.outro_start_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_library::repo;
    use std::sync::Mutex;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures/sine_opus.webm")
    }

    #[test]
    fn analyses_upcoming_first_then_the_rest_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        std::fs::create_dir_all(dir.path().join("music")).unwrap();
        std::fs::copy(fixture(), dir.path().join("music/a.webm")).unwrap();
        db.write(|t| {
            for id in ["a", "b", "missing"] {
                repo::upsert_track(
                    t,
                    &repo::TrackMeta {
                        id: TrackId::from(id),
                        title: id.into(),
                        artists: vec![],
                        album: None,
                        track_no: None,
                        duration_ms: 2000,
                        cover_key: None,
                    },
                )?;
            }
            for (id, file) in [("a", "music/a.webm"), ("b", "music/a.webm"), ("missing", "music/x.webm")] {
                t.execute(
                    "INSERT INTO media_files(track_id, path, container, size_bytes, last_access, location)
                     VALUES (?1, ?2, 'webm', 1, 0, 'library')",
                    [id, file],
                )?;
            }
            Ok(())
        })
        .unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let done_tx = Mutex::new(done_tx);
        let analyzer = Analyzer::start(
            db.clone(),
            dir.path().to_path_buf(),
            AnalyzerHooks {
                upcoming: Box::new(|| vec![TrackId::from("missing")]),
                analysed: Box::new(move |id| {
                    let _ = done_tx.lock().unwrap().send(id.0.clone());
                }),
                on_start: Box::new(|| {}),
            },
        )
        .unwrap();
        analyzer.kick();
        // Disabled: nothing happens.
        assert!(done_rx.recv_timeout(Duration::from_millis(300)).is_err());
        analyzer.set_enabled(true);
        let wait = || done_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let mut order = vec![wait(), wait(), wait()];
        assert_eq!(order[0], "missing", "upcoming first");
        order.sort();
        assert_eq!(order, ["a", "b", "missing"]);
        assert!(
            done_rx.recv_timeout(Duration::from_millis(800)).is_err(),
            "each once"
        );

        let get = |id: &str| {
            let id = TrackId::from(id);
            db.read(|c| analysis::get(c, &id)).unwrap().unwrap()
        };
        let a = to_audio(&get("a")).unwrap();
        assert!(a.usable() && a.end_ms.abs_diff(2000) <= 250, "{a:?}");
        assert!(
            !to_audio(&get("missing")).unwrap().usable(),
            "failures are stored as unusable"
        );
    }

    #[test]
    fn stored_and_audio_forms_round_trip() {
        let a = TrackAnalysis {
            bpm: 128.0,
            beat_ms: 12.5,
            key: Some(Key::new(9, true)),
            start_ms: 10,
            end_ms: 200_000,
            intro_end_ms: 15_000,
            outro_start_ms: 180_000,
        };
        assert_eq!(to_audio(&to_stored(&a)), Some(a));
        let mut old = to_stored(&TrackAnalysis::empty());
        old.version = ANALYSIS_VERSION + 1;
        assert_eq!(to_audio(&old), None);
    }
}
