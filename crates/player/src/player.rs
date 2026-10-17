//! The player: owns the queue and the audio engine, turns engine events into state the
//! UI shows, keeps the next track lined up for gapless playback and remembers where the
//! user stopped.

use crate::mix::{MixPage, MixSession, MoreRequest};
use crate::prefetch::{self, StreamProvider};
use crate::queue::{Queue, QueueContext, Repeat};
use crate::state::{PlaybackState, QueueEntry, QueueView, Status};
use mixtape_audio::{Engine, EngineEvent, Source};
use mixtape_core::{AppError, AppResult, ErrorKind, Track, TrackId};
use mixtape_library::{Db, repo, settings};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

/// Notifications for the shell (Tauri) to forward to the UI and to Windows.
#[derive(Debug, Clone)]
pub enum PlayerEvent {
    State(PlaybackState),
    TrackChanged { track: Track, palette: Vec<String> },
    QueueChanged,
    /// A play finished: `full` when it counts as a complete listen.
    Played { track_id: TrackId, full: bool, full_plays: u32 },
    Error(String),
}

pub type EventSink = Arc<dyn Fn(PlayerEvent) + Send + Sync>;

/// Stop skipping after this many tracks failed in a row.
const MAX_SKIPS: u32 = 3;
const SNAPSHOT_KEY: &str = "queue_snapshot";
/// Upper bound for the blend on next/previous/jump, so skipping still feels instant.
const MANUAL_FADE_MS: u32 = 2_000;
const SNAPSHOT_EVERY: Duration = Duration::from_secs(2);

#[derive(Serialize, Deserialize)]
struct Snapshot {
    queue: Queue,
    position_ms: u64,
    volume: f32,
    #[serde(default)]
    mix: Option<MixSession>,
}

pub struct Player {
    me: Weak<Player>,
    inner: Mutex<Inner>,
    streams: Option<Arc<dyn StreamProvider>>,
    engine: Engine,
    db: Db,
    data_root: PathBuf,
    sink: EventSink,
}

struct Inner {
    queue: Queue,
    state: PlaybackState,
    /// Track handed to the engine as the gapless successor.
    pending_next: Option<TrackId>,
    /// Position the current play started from, to judge "full listen".
    seeked_forward: bool,
    skips: u32,
    last_snapshot: Instant,
    /// Crossfade for automatic transitions, ms; 0 = gapless.
    crossfade_ms: u32,
    mix: Option<MixSession>,
    /// Bumped on every load, so a stream that resolves late does not replace a newer track.
    load_seq: u64,
    /// A streamed track restored paused at startup: loaded on the first play, so launching
    /// the app does not start a download.
    deferred: Option<(TrackId, u64)>,
    /// The remote successor whose stream is being opened.
    next_request: Option<TrackId>,
}

fn now_ms() -> u64 {
    repo::now_ms() as u64
}

fn err(kind: ErrorKind, msg: impl Into<String>) -> AppError {
    AppError::new(kind, msg)
}

impl Player {
    pub fn start(
        db: Db,
        data_root: PathBuf,
        sink: EventSink,
        streams: Option<Arc<dyn StreamProvider>>,
    ) -> AppResult<Arc<Self>> {
        let (engine, events) =
            Engine::start().map_err(|e| err(ErrorKind::Audio, format!("Не удалось запустить звук: {e}")))?;
        let snapshot: Option<Snapshot> = settings::get(&db, SNAPSHOT_KEY).ok().flatten();
        let (queue, position_ms, volume, mix) = match snapshot {
            Some(s) => (s.queue, s.position_ms, s.volume, s.mix),
            None => (Queue::new(), 0, 0.8, None),
        };
        let state = PlaybackState {
            track_id: queue.current().map(|i| i.track_id.clone()),
            status: if queue.current().is_some() { Status::Paused } else { Status::Idle },
            position_ms,
            volume,
            repeat: queue.repeat(),
            shuffle: queue.shuffle(),
            ..Default::default()
        };
        engine.set_volume(volume);
        let player = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            streams,
            inner: Mutex::new(Inner {
                queue,
                state,
                pending_next: None,
                seeked_forward: false,
                skips: 0,
                last_snapshot: Instant::now(),
                crossfade_ms: 0,
                mix,
                load_seq: 0,
                deferred: None,
                next_request: None,
            }),
            engine,
            db,
            data_root,
            sink,
        });
        // Restore the last track paused at its position, without making a sound.
        // Take the id first: a guard in the `if let` scrutinee would stay locked through
        // the body, and `load_track` locks again.
        let restored = player.lock().queue.current().map(|i| i.track_id.clone());
        if let Some(id) = restored {
            player.load_track(&id, position_ms, false);
        }
        let weak = Arc::downgrade(&player);
        std::thread::Builder::new()
            .name("player-events".into())
            .spawn(move || {
                while let Ok(ev) = events.recv() {
                    let Some(p) = weak.upgrade() else { break };
                    p.on_engine(ev);
                }
            })
            .map_err(|e| err(ErrorKind::Internal, e.to_string()))?;
        Ok(player)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("player state poisoned")
    }

    pub fn state(&self) -> PlaybackState {
        self.lock().state.clone()
    }

    pub fn current_track(&self) -> Option<Track> {
        let id = self.lock().state.track_id.clone()?;
        self.db.read(|c| repo::track(c, &id)).ok().flatten()
    }

    fn emit_state(&self, inner: &mut Inner) {
        inner.state.timestamp_ms = now_ms();
        (self.sink)(PlayerEvent::State(inner.state.clone()));
    }

    /// Builds an engine source for a track that has audio on disk.
    fn source(&self, id: &TrackId) -> AppResult<Source> {
        let media = self
            .db
            .read(|c| repo::media_file(c, id))
            .map_err(AppError::from)?
            .ok_or_else(|| err(ErrorKind::NotFound, "Трек ещё не скачан"))?;
        let gain = mixtape_audio::gain_for(media.lufs, media.true_peak);
        let mut src = Source::file(id.as_str(), self.data_root.join(&media.path), gain);
        src.hint_ext = Some(media.container);
        Ok(src)
    }

    fn has_audio(&self, id: &TrackId) -> bool {
        self.db.read(|c| repo::media_file(c, id)).ok().flatten().is_some()
    }

    /// Loads `id` into the engine as the current track and announces it. A track without
    /// audio on disk streams: it is announced now and reaches the engine once its URL
    /// resolves.
    fn load_track(&self, id: &TrackId, start_ms: u64, play: bool) -> bool {
        let src = match self.source(id) {
            Ok(src) => Some(src),
            Err(e) if e.kind == ErrorKind::NotFound && self.streams.is_some() => None,
            Err(e) => {
                (self.sink)(PlayerEvent::Error(format!("Не удалось воспроизвести: {}", e.message)));
                return false;
            }
        };
        let streamed = src.is_none();
        let seq = {
            let mut inner = self.lock();
            inner.load_seq += 1;
            inner.deferred = None;
            inner.next_request = None;
            inner.load_seq
        };
        match src {
            Some(src) => self.engine_load(src, start_ms, play),
            None => {
                // Silence the old track while the new one resolves.
                self.engine.stop();
                if play {
                    self.open_stream(id, seq, start_ms);
                } else {
                    self.lock().deferred = Some((id.clone(), start_ms));
                }
            }
        }
        let mut inner = self.lock();
        inner.pending_next = None;
        inner.seeked_forward = start_ms > 0;
        inner.state.track_id = Some(id.clone());
        inner.state.position_ms = start_ms;
        inner.state.status = if play { Status::Loading } else { Status::Paused };
        if let Ok(Some(t)) = self.db.read(|c| repo::track(c, id)) {
            inner.state.duration_ms = t.duration_ms;
            drop(inner);
            self.announce(t);
            let mut inner = self.lock();
            self.emit_state(&mut inner);
        } else {
            self.emit_state(&mut inner);
        }
        if streamed {
            self.prefetch();
        }
        true
    }

    fn engine_load(&self, src: Source, start_ms: u64, play: bool) {
        // A manual switch while music plays blends briefly when crossfade is on.
        let fade = {
            let inner = self.lock();
            let playing = matches!(inner.state.status, Status::Playing | Status::Buffering);
            if play && start_ms == 0 && playing { inner.crossfade_ms.min(MANUAL_FADE_MS) } else { 0 }
        };
        if fade > 0 {
            self.engine.crossfade_to(src, fade);
        } else {
            self.engine.load(src, start_ms, play);
        }
    }

    /// Asks the provider for the stream of the current track; a late answer for a track the
    /// user already left is dropped.
    fn open_stream(&self, id: &TrackId, seq: u64, start_ms: u64) {
        let Some(streams) = &self.streams else { return };
        let me = self.me.clone();
        let id2 = id.clone();
        streams.open(
            id,
            Box::new(move |res| {
                let Some(p) = me.upgrade() else { return };
                if p.lock().load_seq != seq {
                    return;
                }
                match res {
                    Ok(src) => {
                        let at = p.lock().state.position_ms.max(start_ms);
                        p.engine.load(src, at, true);
                    }
                    Err(message) => p.on_engine(EngineEvent::Error { id: id2.0, message }),
                }
            }),
        );
    }

    /// Tells the provider which remote tracks come next.
    fn prefetch(&self) {
        let Some(streams) = &self.streams else { return };
        let (current, window) = {
            let inner = self.lock();
            (inner.state.track_id.clone(), prefetch::window(&inner.queue))
        };
        let upcoming = prefetch::upcoming_remote(window, |id| self.has_audio(id));
        streams.prefetch(current.as_ref(), &upcoming);
    }

    fn announce(&self, track: Track) {
        let palette = track
            .album
            .as_ref()
            .and_then(|a| self.db.read(|c| repo::album(c, &a.id)).ok().flatten())
            .map(|a| a.palette)
            .unwrap_or_default();
        (self.sink)(PlayerEvent::TrackChanged { track, palette });
    }

    /// Starts the current queue item, skipping forward over tracks that cannot play.
    fn start_current(&self, start_ms: u64) {
        for _ in 0..=MAX_SKIPS {
            let Some(id) = self.lock().queue.current().map(|i| i.track_id.clone()) else { return };
            if self.load_track(&id, start_ms, true) {
                return;
            }
            if self.lock().queue.advance(false).is_none() {
                break;
            }
        }
        self.stop_idle();
    }

    fn stop_idle(&self) {
        self.engine.stop();
        let mut inner = self.lock();
        inner.state.status = Status::Idle;
        self.emit_state(&mut inner);
    }

    // ---------- commands ----------

    pub fn play_tracks(&self, ids: Vec<TrackId>, start: usize, context: QueueContext) {
        if ids.is_empty() {
            return;
        }
        {
            let mut inner = self.lock();
            inner.queue.set(ids, start, context);
            inner.skips = 0;
            inner.mix = None;
        }
        (self.sink)(PlayerEvent::QueueChanged);
        self.start_current(0);
        self.save_snapshot(true);
    }

    // ---------- mixes ----------

    /// Replaces the queue with a mix's first page. A seed that is already the current track
    /// keeps playing without a restart.
    pub fn start_mix(&self, seed: TrackId, page: MixPage) {
        let (session, tracks) = MixSession::start(seed.clone(), page);
        let keep_playing = {
            let mut inner = self.lock();
            let keep = inner.state.track_id.as_ref() == Some(&seed)
                && matches!(inner.state.status, Status::Playing | Status::Buffering | Status::Loading);
            inner.queue.set(tracks, 0, QueueContext::Mix(seed));
            inner.skips = 0;
            inner.mix = Some(session);
            keep
        };
        (self.sink)(PlayerEvent::QueueChanged);
        if keep_playing {
            self.refresh_next();
        } else {
            self.start_current(0);
        }
        self.save_snapshot(true);
    }

    /// Tracks after the current one.
    pub fn remaining(&self) -> usize {
        self.lock().queue.remaining()
    }

    /// The current track and the tracks already in the queue, for an offline continuation.
    pub fn queued_ids(&self) -> Vec<TrackId> {
        self.lock().queue.items().iter().map(|i| i.track_id.clone()).collect()
    }

    pub fn mix(&self) -> Option<MixSession> {
        self.lock().mix.clone()
    }

    /// The continuation to fetch when the mix is running low.
    pub fn mix_begin_more(&self) -> Option<MoreRequest> {
        let mut inner = self.lock();
        let remaining = inner.queue.remaining();
        inner.mix.as_mut()?.begin_more(remaining)
    }

    pub fn mix_accept_more(&self, generation: u64, page: MixPage) {
        let added = match self.lock().mix.as_mut() {
            Some(mix) => mix.accept_more(generation, page),
            None => return,
        };
        if !added.is_empty() {
            self.edit_queue(|q| q.append_mix(added));
        }
    }

    pub fn mix_more_failed(&self, generation: u64) {
        if let Some(mix) = self.lock().mix.as_mut() {
            mix.more_failed(generation);
        }
    }

    /// Marks a chip selected; returns the seed, the chip and the generation its page belongs to.
    pub fn mix_select_chip(&self, chip_id: &str) -> Option<(TrackId, crate::mix::MixChip, u64)> {
        let picked = {
            let mut inner = self.lock();
            let mix = inner.mix.as_mut()?;
            let generation = mix.select_chip(chip_id)?;
            (mix.seed.clone(), mix.chip(chip_id)?.clone(), generation)
        };
        (self.sink)(PlayerEvent::QueueChanged);
        Some(picked)
    }

    /// Replaces what is ahead in the mix with the chip's page; the current track plays on.
    pub fn mix_accept_chip(&self, generation: u64, page: MixPage) {
        let added = {
            let mut inner = self.lock();
            let cur = inner.queue.current_index().unwrap_or(0);
            let kept: Vec<TrackId> = inner.queue.items().iter().take(cur + 1).map(|i| i.track_id.clone()).collect();
            match inner.mix.as_mut() {
                Some(mix) if mix.generation() == generation => mix.accept_chip(generation, &kept, page),
                _ => return,
            }
        };
        self.edit_queue(|q| q.replace_upcoming_mix(added));
    }

    pub fn toggle(&self) {
        let (status, has_track) = {
            let inner = self.lock();
            (inner.state.status, inner.queue.current().is_some())
        };
        match status {
            Status::Playing | Status::Buffering | Status::Loading => self.pause(),
            _ if has_track => self.play(),
            _ => {}
        }
    }

    pub fn play(&self) {
        let deferred = self.lock().deferred.take();
        if let Some((id, at)) = deferred {
            self.load_track(&id, at, true);
            return;
        }
        self.engine.play();
        let mut inner = self.lock();
        if inner.state.status != Status::Playing {
            inner.state.status = Status::Playing;
            self.emit_state(&mut inner);
        }
    }

    pub fn pause(&self) {
        self.engine.pause();
        let mut inner = self.lock();
        inner.state.status = Status::Paused;
        self.emit_state(&mut inner);
        drop(inner);
        self.save_snapshot(true);
    }

    pub fn next(&self) {
        self.record_end(false);
        let moved = self.lock().queue.advance(false).is_some();
        if moved {
            (self.sink)(PlayerEvent::QueueChanged);
            self.start_current(0);
        }
    }

    /// Previous track in the queue. Never restarts the current one; with nothing before
    /// it (and repeat off) it does nothing.
    pub fn prev(&self) {
        let moved = self.lock().queue.back().is_some();
        if moved {
            (self.sink)(PlayerEvent::QueueChanged);
            self.start_current(0);
        }
    }

    pub fn seek(&self, ms: u64) {
        self.engine.seek(ms);
        let mut inner = self.lock();
        if ms > inner.state.position_ms + 5_000 {
            inner.seeked_forward = true;
        }
        inner.state.position_ms = ms;
        self.emit_state(&mut inner);
    }

    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        self.engine.set_volume(v);
        let mut inner = self.lock();
        inner.state.volume = v;
        self.emit_state(&mut inner);
        drop(inner);
        self.save_snapshot(false);
    }

    /// Crossfade length for automatic transitions; 0 keeps playback gapless.
    pub fn set_crossfade(&self, ms: u32) {
        self.lock().crossfade_ms = ms;
        self.engine.set_crossfade(ms);
        self.refresh_next();
    }

    pub fn set_repeat(&self, r: Repeat) {
        let mut inner = self.lock();
        inner.queue.set_repeat(r);
        inner.state.repeat = r;
        self.emit_state(&mut inner);
        drop(inner);
        self.refresh_next();
    }

    pub fn set_shuffle(&self, on: bool) {
        let mut inner = self.lock();
        inner.queue.set_shuffle(on);
        inner.state.shuffle = on;
        self.emit_state(&mut inner);
        drop(inner);
        (self.sink)(PlayerEvent::QueueChanged);
        self.refresh_next();
    }

    /// Runs a queue edit, then re-lines up the gapless successor.
    pub fn edit_queue<R>(&self, f: impl FnOnce(&mut Queue) -> R) -> R {
        let (r, was_empty, now_has) = {
            let mut inner = self.lock();
            let was_empty = inner.queue.current().is_none();
            let r = f(&mut inner.queue);
            (r, was_empty, inner.queue.current().is_some())
        };
        (self.sink)(PlayerEvent::QueueChanged);
        if was_empty && now_has {
            self.start_current(0);
        } else {
            self.refresh_next();
        }
        self.save_snapshot(true);
        r
    }

    pub fn jump(&self, uid: u64) {
        self.record_end(false);
        let ok = self.lock().queue.jump(uid).is_some();
        if ok {
            (self.sink)(PlayerEvent::QueueChanged);
            self.start_current(0);
        }
    }

    pub fn queue_view(&self) -> QueueView {
        let inner = self.lock();
        let ids: Vec<TrackId> = inner.queue.items().iter().map(|i| i.track_id.clone()).collect();
        let tracks = self.db.read(|c| repo::tracks_by_ids(c, &ids)).unwrap_or_default();
        let by_id: std::collections::HashMap<&str, &Track> =
            tracks.iter().map(|t| (t.id.as_str(), t)).collect();
        let items = inner
            .queue
            .items()
            .iter()
            .filter_map(|i| {
                by_id.get(i.track_id.as_str()).map(|t| QueueEntry {
                    uid: i.uid,
                    origin: i.origin,
                    track: (*t).clone(),
                })
            })
            .collect();
        QueueView {
            items,
            current: inner.queue.current_index().map(|i| i as u32),
            context: inner.queue.context().clone(),
            chips: inner.mix.as_ref().map(MixSession::chips),
        }
    }

    /// Tells the engine what follows the current track (or that nothing does). A remote
    /// successor is lined up once its stream opens.
    fn refresh_next(&self) {
        let next = {
            let inner = self.lock();
            inner.queue.peek_next(true).map(|i| i.track_id.clone())
        };
        let mut src = None;
        if let Some(id) = &next {
            match self.source(id) {
                Ok(s) => src = Some(s),
                Err(e) if e.kind == ErrorKind::NotFound && self.streams.is_some() => {
                    self.request_next(id);
                    self.prefetch();
                    return;
                }
                Err(_) => {}
            }
        }
        // Consecutive tracks of one album stay gapless even with crossfade on.
        if let (Some(s), Some(next_id)) = (src.as_mut(), next.as_ref()) {
            let current = self.lock().state.track_id.clone();
            let album_of = |id: &TrackId| {
                self.db.read(|c| repo::track(c, id)).ok().flatten().and_then(|t| t.album.map(|a| a.id))
            };
            let same_album = current.as_ref().and_then(album_of).is_some_and(|a| Some(a) == album_of(next_id));
            s.crossfade = !same_album;
        }
        let mut inner = self.lock();
        inner.pending_next = src.as_ref().and(next);
        inner.next_request = None;
        drop(inner);
        self.engine.set_next(src);
        self.prefetch();
    }

    /// Opens the stream of the remote successor and hands it to the engine if it is still next.
    fn request_next(&self, id: &TrackId) {
        let Some(streams) = &self.streams else { return };
        {
            let mut inner = self.lock();
            if inner.next_request.as_ref() == Some(id) || inner.pending_next.as_ref() == Some(id) {
                return;
            }
            inner.next_request = Some(id.clone());
        }
        let me = self.me.clone();
        let id2 = id.clone();
        streams.open(
            id,
            Box::new(move |res| {
                let Some(p) = me.upgrade() else { return };
                let Ok(src) = res else { return };
                let mut inner = p.lock();
                let still_next = inner.next_request.as_ref() == Some(&id2)
                    && inner.queue.peek_next(true).map(|i| &i.track_id) == Some(&id2);
                if !still_next {
                    return;
                }
                inner.next_request = None;
                inner.pending_next = Some(id2);
                drop(inner);
                p.engine.set_next(Some(src));
            }),
        );
    }

    // ---------- engine events ----------

    fn on_engine(&self, ev: EngineEvent) {
        match ev {
            EngineEvent::Started { id, duration_ms } => {
                let id = TrackId(id);
                let advanced = {
                    let mut inner = self.lock();
                    inner.skips = 0;
                    // `load` clears `pending_next`, so a match here can only be the
                    // gapless successor (including the same track under repeat-one).
                    let gapless = inner.pending_next.as_ref() == Some(&id);
                    if gapless {
                        inner.queue.advance(true);
                        inner.pending_next = None;
                        inner.seeked_forward = false;
                        inner.state.position_ms = 0;
                    }
                    inner.state.track_id = Some(id.clone());
                    if duration_ms > 0 {
                        inner.state.duration_ms = duration_ms;
                    }
                    inner.state.status = Status::Playing;
                    self.emit_state(&mut inner);
                    gapless
                };
                if advanced {
                    if let Ok(Some(t)) = self.db.read(|c| repo::track(c, &id)) {
                        self.announce(t);
                    }
                    (self.sink)(PlayerEvent::QueueChanged);
                }
                self.refresh_next();
            }
            EngineEvent::Position { id, ms } => {
                let mut inner = self.lock();
                if inner.state.track_id.as_ref().map(|t| t.as_str()) == Some(id.as_str()) {
                    inner.state.position_ms = ms;
                    self.emit_state(&mut inner);
                }
                drop(inner);
                self.save_snapshot(false);
            }
            EngineEvent::StateChanged { playing } => {
                let mut inner = self.lock();
                inner.state.status = if playing { Status::Playing } else { Status::Paused };
                self.emit_state(&mut inner);
            }
            EngineEvent::Buffering(on) => {
                let mut inner = self.lock();
                inner.state.status = if on { Status::Buffering } else { Status::Playing };
                self.emit_state(&mut inner);
            }
            EngineEvent::NeedNext { .. } => self.refresh_next(),
            EngineEvent::Ended { id, played_ms, completed } => {
                let id = TrackId(id);
                let (full, last, stalled) = {
                    let inner = self.lock();
                    let dur = inner.state.duration_ms.max(1);
                    let full = !inner.seeked_forward && (completed || played_ms * 10 >= dur * 9);
                    let ahead = inner.queue.peek_next(true).is_some();
                    let current = inner.state.track_id.as_ref() == Some(&id);
                    let last = completed && inner.pending_next.is_none() && !ahead;
                    // The successor was still resolving when this track ran out.
                    let stalled = completed && current && inner.pending_next.is_none() && ahead;
                    (full, last, stalled)
                };
                self.count_play(&id, full);
                if stalled {
                    self.lock().queue.advance(true);
                    (self.sink)(PlayerEvent::QueueChanged);
                    self.start_current(0);
                }
                if last {
                    let mut inner = self.lock();
                    inner.state.status = Status::Idle;
                    inner.state.position_ms = 0;
                    self.emit_state(&mut inner);
                }
            }
            EngineEvent::Error { id, message } => {
                tracing::warn!(%id, %message, "playback error");
                (self.sink)(PlayerEvent::Error(format!("Ошибка воспроизведения: {message}")));
                let skip = {
                    let mut inner = self.lock();
                    inner.skips += 1;
                    inner.skips <= MAX_SKIPS && inner.queue.advance(false).is_some()
                };
                if skip {
                    (self.sink)(PlayerEvent::QueueChanged);
                    self.start_current(0);
                } else {
                    self.stop_idle();
                }
            }
        }
    }

    /// Counts the current track as played when the user leaves it early.
    fn record_end(&self, full: bool) {
        let (id, pos) = {
            let inner = self.lock();
            (inner.state.track_id.clone(), inner.state.position_ms)
        };
        if let Some(id) = id
            && pos > 30_000
        {
            self.count_play(&id, full);
        }
    }

    fn count_play(&self, id: &TrackId, full: bool) {
        let id2 = id.clone();
        if let Ok(full_plays) = self.db.write(move |t| repo::record_play(t, &id2, full)) {
            (self.sink)(PlayerEvent::Played { track_id: id.clone(), full, full_plays });
        }
    }

    /// Persists queue, position and volume; throttled unless `force`.
    pub fn save_snapshot(&self, force: bool) {
        let json = {
            let mut inner = self.lock();
            if !force && inner.last_snapshot.elapsed() < SNAPSHOT_EVERY {
                return;
            }
            inner.last_snapshot = Instant::now();
            Snapshot {
                queue: inner.queue.clone(),
                position_ms: inner.state.position_ms,
                volume: inner.state.volume,
                mix: inner.mix.clone(),
            }
        };
        if let Err(e) = settings::set(&self.db, SNAPSHOT_KEY, &json) {
            tracing::warn!(error = %e, "saving queue snapshot failed");
        }
    }
}
