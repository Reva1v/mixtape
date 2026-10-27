//! The player: owns the queue and the audio engine, turns engine events into state the
//! UI shows, keeps the next track lined up for gapless playback and remembers where the
//! user stopped.

use crate::listen::{Listen, Listens};
use crate::mix::{MixPage, MixSession, MoreRequest};
use crate::prefetch::{self, StreamProvider};
use crate::queue::{Queue, QueueContext, Repeat};
use crate::state::{PlaybackState, QueueEntry, QueueView, Status};
use mixtape_audio::{Engine, EngineEvent, Source, TrackAnalysis};
use mixtape_core::{AppError, AppResult, ErrorKind, Track, TrackId, tr, trf};
use mixtape_library::{Db, analysis, repo, settings, stats};
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
    /// A play was added to the listening log (`play_events`).
    Listened(Listen),
    Error(String),
}

pub type EventSink = Arc<dyn Fn(PlayerEvent) + Send + Sync>;

/// Stop skipping after this many tracks failed in a row.
const MAX_SKIPS: u32 = 3;
/// The queue and mix; written only when they change.
const SNAPSHOT_KEY: &str = "queue_snapshot";
/// Where in the current track playback is, and the volume; small, written often.
const POSITION_KEY: &str = "playback_pos";
/// Upper bound for the blend on next/previous/jump, so skipping still feels instant.
const MANUAL_FADE_MS: u32 = 2_000;
/// The position is saved this often while playing (and on pause, stop and exit).
const POSITION_SAVE_EVERY: Duration = Duration::from_secs(20);
/// A volume change or seek is saved at most this often.
const POSITION_SAVE_CHANGED: Duration = Duration::from_secs(2);
/// While playing the UI extrapolates the position itself; state goes out on every change
/// and, to correct drift, at least this often.
const STATE_EVERY: Duration = Duration::from_secs(5);

/// The queue, position, volume and mix: what is saved under `queue_snapshot`, and what a
/// hand-off between the app and the island carries (`mixtape_session::handoff::Handoff`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub queue: Queue,
    /// Position when the queue was saved; superseded by [`PlaybackPos`] for the same item.
    pub position_ms: u64,
    pub volume: f32,
    #[serde(default)]
    pub mix: Option<MixSession>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PlaybackPos {
    /// Queue item the position belongs to.
    #[serde(default)]
    uid: Option<u64>,
    #[serde(default)]
    track_id: Option<TrackId>,
    position_ms: u64,
    volume: f32,
}

/// Queue, position, volume and mix to start with. The position comes from `pos` when it
/// belongs to the snapshot's current item (snapshots written before `playback_pos` existed
/// carry their own).
fn restore(snapshot: Option<Snapshot>, pos: Option<PlaybackPos>) -> (Queue, u64, f32, Option<MixSession>) {
    let (queue, mut position_ms, mut volume, mix) = match snapshot {
        Some(s) => (s.queue, s.position_ms, s.volume, s.mix.map(MixSession::restored)),
        None => (Queue::new(), 0, 0.8, None),
    };
    if let Some(p) = pos {
        volume = p.volume;
        if p.uid.is_some() && p.uid == queue.current().map(|i| i.uid) {
            position_ms = p.position_ms;
        }
    }
    (queue, position_ms, volume, mix)
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
    /// The queue or mix changed since `queue_snapshot` was written.
    queue_dirty: bool,
    /// Volume or position changed by the user since `playback_pos` was written.
    position_dirty: bool,
    last_position_save: Instant,
    /// When `state.position_ms` was last set; it is extrapolated from there while playing.
    position_at: Instant,
    last_emit: Instant,
    /// The last `StreamProvider::prefetch` arguments, so an unchanged window is not re-sent.
    last_prefetch: Option<(Option<TrackId>, Vec<TrackId>)>,
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
    /// A successor the engine failed to open: not lined up again, so the current track
    /// plays to its end and the failure is handled when it becomes current.
    failed_next: Option<TrackId>,
    /// Listening time of the plays in progress, for the statistics log.
    listens: Listens,
}

impl Inner {
    /// Where playback is now: positions arrive about once a second, so it is extrapolated
    /// while playing.
    fn live_position(&self) -> u64 {
        let ms = self.state.position_ms;
        if self.state.status != Status::Playing {
            return ms;
        }
        let ms = ms + self.position_at.elapsed().as_millis() as u64;
        if self.state.duration_ms > 0 { ms.min(self.state.duration_ms) } else { ms }
    }

    fn set_position(&mut self, ms: u64) {
        self.state.position_ms = ms;
        self.position_at = Instant::now();
    }

    fn set_status(&mut self, status: Status) {
        if status != self.state.status {
            let ms = self.live_position();
            self.state.status = status;
            self.set_position(ms);
        }
    }

    fn live_state(&self) -> PlaybackState {
        PlaybackState { position_ms: self.live_position(), ..self.state.clone() }
    }
}

/// An engine error about `id` concerns the lined-up successor, not the playing track.
fn is_successor_error(id: &str, current: Option<&TrackId>, pending: Option<&TrackId>, next: Option<&TrackId>) -> bool {
    current.is_some_and(|t| t.as_str() != id)
        && (pending.is_some_and(|t| t.as_str() == id) || next.is_some_and(|t| t.as_str() == id))
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
            Engine::start().map_err(|e| {
                err(
                    ErrorKind::Audio,
                    trf!("Couldn't start the audio: {e}", "Не вдалося запустити звук: {e}", "Не удалось запустить звук: {e}"),
                )
            })?;
        let snapshot: Option<Snapshot> = settings::get(&db, SNAPSHOT_KEY).ok().flatten();
        let pos: Option<PlaybackPos> = settings::get(&db, POSITION_KEY).ok().flatten();
        let (queue, position_ms, volume, mix) = restore(snapshot, pos);
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
                queue_dirty: false,
                position_dirty: false,
                last_position_save: Instant::now(),
                position_at: Instant::now(),
                last_emit: Instant::now(),
                last_prefetch: None,
                crossfade_ms: 0,
                mix,
                load_seq: 0,
                deferred: None,
                next_request: None,
                failed_next: None,
                listens: Listens::default(),
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
        PlaybackState { timestamp_ms: now_ms(), ..self.lock().live_state() }
    }

    pub fn current_track(&self) -> Option<Track> {
        let id = self.lock().state.track_id.clone()?;
        self.db.read(|c| repo::track(c, &id)).ok().flatten()
    }

    fn emit_state(&self, inner: &mut Inner) {
        inner.state.timestamp_ms = now_ms();
        inner.last_emit = Instant::now();
        (self.sink)(PlayerEvent::State(inner.live_state()));
    }

    /// Builds an engine source for a track that has audio on disk.
    fn source(&self, id: &TrackId) -> AppResult<Source> {
        let media = self
            .db
            .read(|c| repo::media_file(c, id))
            .map_err(AppError::from)?
            .ok_or_else(|| err(ErrorKind::NotFound, tr("The track isn't downloaded yet", "Трек ще не завантажено", "Трек ещё не скачан")))?;
        let mut src = Source::file(id.as_str(), self.data_root.join(&media.path), self.track_gain(id));
        src.hint_ext = Some(media.container);
        src.analysis = self.track_analysis(id);
        Ok(src)
    }

    /// The stored analysis of a track for smart transitions, if it is current and usable.
    fn track_analysis(&self, id: &TrackId) -> Option<Arc<TrackAnalysis>> {
        let stored = self.db.read(|c| analysis::get(c, id)).ok().flatten()?;
        let a = crate::analyzer::to_audio(&stored)?;
        a.usable().then(|| Arc::new(a))
    }

    /// The analysis of `id` was stored: a playing or queued source of it picks it up.
    pub fn refresh_analysis(&self, id: &TrackId) {
        // Like `refresh_gain`: the stream callbacks hand sources over under this lock.
        let _inner = self.lock();
        self.engine.set_analysis(id.as_str(), self.track_analysis(id));
    }

    /// Smart transitions («Умные переходы») on or off; off, the crossfade setting applies.
    pub fn set_smart_transitions(&self, on: bool) {
        self.engine.set_smart(on);
    }

    /// The current track and the one that plays after it, if any.
    pub fn upcoming(&self) -> Vec<TrackId> {
        let inner = self.lock();
        inner.queue.current().into_iter().chain(inner.queue.peek_next(true)).map(|i| i.track_id.clone()).collect()
    }

    /// The audio file of `id` moved (cache → library, or a download finished). A successor
    /// lined up from the old path is lined up again, so it does not fail when it is due.
    pub fn media_moved(&self, id: &TrackId) {
        let lined_up = {
            let inner = self.lock();
            inner.pending_next.as_ref() == Some(id)
                || inner.failed_next.as_ref() == Some(id)
                || inner.queue.peek_next(true).is_some_and(|i| &i.track_id == id)
        };
        if lined_up {
            self.lock().failed_next = None;
            self.refresh_next();
        }
    }

    /// Normalisation gain of a track from what is known about its loudness (0 dB if nothing).
    fn track_gain(&self, id: &TrackId) -> f32 {
        let l = self.db.read(|c| repo::track_loudness(c, id)).unwrap_or_default();
        mixtape_audio::gain_for(l.lufs, l.true_peak)
    }

    /// The loudness of `id` became known (YouTube answered, or its file was measured): a
    /// playing or queued source of it glides to the new gain.
    pub fn refresh_gain(&self, id: &TrackId) {
        // The stream callbacks read the gain and hand the source over under this lock.
        let _inner = self.lock();
        self.engine.set_gain(id.as_str(), self.track_gain(id));
    }

    /// Loudness normalisation on or off; applies to the playing track at once.
    pub fn set_normalization(&self, on: bool) {
        self.engine.set_normalization(on);
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
                let m = e.message;
                (self.sink)(PlayerEvent::Error(trf!(
                    "Couldn't play: {m}",
                    "Не вдалося відтворити: {m}",
                    "Не удалось воспроизвести: {m}",
                )));
                return false;
            }
        };
        let streamed = src.is_none();
        let seq = {
            let mut inner = self.lock();
            inner.load_seq += 1;
            inner.deferred = None;
            inner.next_request = None;
            inner.failed_next = None;
            // A new track: let the provider hear about the window again (retries failures).
            inner.last_prefetch = None;
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
        inner.set_status(if play { Status::Loading } else { Status::Paused });
        inner.set_position(start_ms);
        if let Ok(Some(t)) = self.db.read(|c| repo::track(c, id)) {
            inner.state.duration_ms = t.duration_ms;
            drop(inner);
            self.announce(t);
            let mut inner = self.lock();
            self.emit_state(&mut inner);
        } else {
            self.emit_state(&mut inner);
            // `prefetch` takes the lock again.
            drop(inner);
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
                    Ok(mut src) => {
                        // Under the lock, so a `refresh_gain` cannot slip in between.
                        let inner = p.lock();
                        src.gain_db = p.track_gain(&id2);
                        src.analysis = p.track_analysis(&id2);
                        let at = inner.live_position().max(start_ms);
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
        {
            let mut inner = self.lock();
            let call = (current, upcoming);
            if inner.last_prefetch.as_ref() == Some(&call) {
                return;
            }
            inner.last_prefetch = Some(call.clone());
            drop(inner);
            streams.prefetch(call.0.as_ref(), &call.1);
        }
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
    /// Callers moved the queue's current item (or replaced the queue), so the queue snapshot
    /// is saved here.
    fn start_current(&self, start_ms: u64) {
        self.lock().queue_dirty = true;
        for _ in 0..=MAX_SKIPS {
            let Some(id) = self.lock().queue.current().map(|i| i.track_id.clone()) else {
                self.save_snapshot(false);
                return;
            };
            if self.load_track(&id, start_ms, true) {
                self.save_snapshot(false);
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
        inner.set_status(Status::Idle);
        self.emit_state(&mut inner);
        drop(inner);
        self.save_snapshot(true);
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
            inner.queue_dirty = true;
        }
        (self.sink)(PlayerEvent::QueueChanged);
        self.start_current(0);
        self.save_snapshot(false);
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
            inner.queue_dirty = true;
            keep
        };
        (self.sink)(PlayerEvent::QueueChanged);
        if keep_playing {
            self.refresh_next();
        } else {
            self.start_current(0);
        }
        self.save_snapshot(false);
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
        let mut inner = self.lock();
        if let Some(mix) = inner.mix.as_mut() {
            mix.more_failed(generation);
            inner.queue_dirty = true;
        }
    }

    /// Marks a chip selected; returns the seed, the chip and the generation its page belongs to.
    pub fn mix_select_chip(&self, chip_id: &str) -> Option<(TrackId, crate::mix::MixChip, u64)> {
        let picked = {
            let mut inner = self.lock();
            let mix = inner.mix.as_mut()?;
            let generation = mix.select_chip(chip_id)?;
            let picked = (mix.seed.clone(), mix.chip(chip_id)?.clone(), generation);
            inner.queue_dirty = true;
            picked
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
            inner.set_status(Status::Playing);
            self.emit_state(&mut inner);
        }
    }

    pub fn pause(&self) {
        self.engine.pause();
        let mut inner = self.lock();
        inner.set_status(Status::Paused);
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
        if ms > inner.live_position() + 5_000 {
            inner.seeked_forward = true;
        }
        if let Some(id) = inner.state.track_id.clone() {
            inner.listens.seek(id.as_str(), ms);
        }
        inner.set_position(ms);
        inner.position_dirty = true;
        self.emit_state(&mut inner);
    }

    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        self.engine.set_volume(v);
        let mut inner = self.lock();
        inner.state.volume = v;
        inner.position_dirty = true;
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
        inner.queue_dirty = true;
        self.emit_state(&mut inner);
        drop(inner);
        self.refresh_next();
        self.save_snapshot(false);
    }

    pub fn set_shuffle(&self, on: bool) {
        let mut inner = self.lock();
        inner.queue.set_shuffle(on);
        inner.state.shuffle = on;
        inner.queue_dirty = true;
        self.emit_state(&mut inner);
        drop(inner);
        (self.sink)(PlayerEvent::QueueChanged);
        self.refresh_next();
        self.save_snapshot(false);
    }

    /// Runs a queue edit, then re-lines up the gapless successor.
    pub fn edit_queue<R>(&self, f: impl FnOnce(&mut Queue) -> R) -> R {
        let (r, before, after, playing) = {
            let mut inner = self.lock();
            let before = inner.queue.current().map(|i| i.uid);
            let r = f(&mut inner.queue);
            inner.queue_dirty = true;
            let after = inner.queue.current().map(|i| (i.uid, i.track_id.clone()));
            (r, before, after, matches!(inner.state.status, Status::Playing | Status::Buffering | Status::Loading))
        };
        (self.sink)(PlayerEvent::QueueChanged);
        match (before, after) {
            (None, Some(_)) => self.start_current(0),
            // The current item itself was removed: what is current now has to be loaded,
            // or the old track would go on playing under the new one's name.
            (Some(was), Some((now, id))) if was != now => {
                self.record_end(false);
                if playing {
                    self.start_current(0);
                } else {
                    self.load_track(&id, 0, false);
                }
            }
            (Some(_), None) => {
                self.record_end(false);
                self.stop_idle();
            }
            _ => self.refresh_next(),
        }
        self.save_snapshot(false);
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
        // The query runs without the lock, so engine events are not held up behind it.
        let ids: Vec<TrackId> = self.lock().queue.items().iter().map(|i| i.track_id.clone()).collect();
        let tracks = self.db.read(|c| repo::tracks_by_ids(c, &ids)).unwrap_or_default();
        let by_id: std::collections::HashMap<&str, &Track> =
            tracks.iter().map(|t| (t.id.as_str(), t)).collect();
        // Items added meanwhile have no row here and are left out until the next refresh.
        let inner = self.lock();
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
            inner.queue.peek_next(true).map(|i| i.track_id.clone()).filter(|id| inner.failed_next.as_ref() != Some(id))
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
            let same_album = current.is_some_and(|cur| {
                self.db
                    .read(|c| Ok((repo::track_album_id(c, &cur)?, repo::track_album_id(c, next_id)?)))
                    .is_ok_and(|(a, b)| a.is_some() && a == b)
            });
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
                let Ok(mut src) = res else { return };
                let mut inner = p.lock();
                let still_next = inner.next_request.as_ref() == Some(&id2)
                    && inner.queue.peek_next(true).map(|i| &i.track_id) == Some(&id2);
                if !still_next {
                    return;
                }
                src.gain_db = p.track_gain(&id2);
                src.analysis = p.track_analysis(&id2);
                inner.next_request = None;
                inner.pending_next = Some(id2);
                p.engine.set_next(Some(src));
            }),
        );
    }

    // ---------- engine events ----------

    fn on_engine(&self, ev: EngineEvent) {
        match ev {
            EngineEvent::Started { id, duration_ms } => {
                let id = TrackId(id);
                let (advanced, closed) = {
                    let mut inner = self.lock();
                    inner.skips = 0;
                    inner.failed_next = None;
                    // `load` clears `pending_next`, so a match here can only be the
                    // gapless successor (including the same track under repeat-one).
                    let gapless = inner.pending_next.as_ref() == Some(&id);
                    if gapless {
                        inner.queue.advance(true);
                        inner.pending_next = None;
                        inner.seeked_forward = false;
                        inner.queue_dirty = true;
                        inner.set_position(0);
                    }
                    inner.state.track_id = Some(id.clone());
                    if duration_ms > 0 {
                        inner.state.duration_ms = duration_ms;
                    }
                    inner.set_status(Status::Playing);
                    self.emit_state(&mut inner);
                    let at = inner.state.position_ms;
                    let closed = inner.listens.start(id.clone(), at, now_ms() as i64);
                    (gapless, closed)
                };
                self.log_listens(closed);
                if advanced {
                    if let Ok(Some(t)) = self.db.read(|c| repo::track(c, &id)) {
                        self.announce(t);
                    }
                    (self.sink)(PlayerEvent::QueueChanged);
                }
                self.refresh_next();
                self.save_snapshot(false);
            }
            EngineEvent::Position { id, ms } => {
                let mut inner = self.lock();
                inner.listens.position(&id, ms);
                if inner.state.track_id.as_ref().map(|t| t.as_str()) == Some(id.as_str()) {
                    inner.set_position(ms);
                    // The UI extrapolates while playing; this only corrects drift.
                    if inner.last_emit.elapsed() >= STATE_EVERY {
                        self.emit_state(&mut inner);
                    }
                }
                drop(inner);
                self.save_snapshot(false);
            }
            EngineEvent::StateChanged { playing } => {
                let mut inner = self.lock();
                inner.set_status(if playing { Status::Playing } else { Status::Paused });
                self.emit_state(&mut inner);
            }
            EngineEvent::Buffering(on) => {
                let mut inner = self.lock();
                inner.set_status(if on { Status::Buffering } else { Status::Playing });
                self.emit_state(&mut inner);
            }
            EngineEvent::NeedNext { .. } => self.refresh_next(),
            EngineEvent::Ended { id, played_ms, completed } => {
                let id = TrackId(id);
                let (full, last, stalled, listened) = {
                    let mut inner = self.lock();
                    let listened = inner.listens.end(id.as_str(), Some(played_ms));
                    let dur = inner.state.duration_ms.max(1);
                    let full = !inner.seeked_forward && (completed || played_ms * 10 >= dur * 9);
                    let ahead = inner.queue.peek_next(true).is_some();
                    let current = inner.state.track_id.as_ref() == Some(&id);
                    let last = completed && inner.pending_next.is_none() && !ahead;
                    // The successor was still resolving when this track ran out.
                    let stalled = completed && current && inner.pending_next.is_none() && ahead;
                    (full, last, stalled, listened)
                };
                self.count_play(&id, full);
                self.log_listens(listened.into_iter().collect());
                if stalled {
                    self.lock().queue.advance(true);
                    (self.sink)(PlayerEvent::QueueChanged);
                    self.start_current(0);
                }
                if last {
                    let mut inner = self.lock();
                    inner.set_status(Status::Idle);
                    inner.set_position(0);
                    self.emit_state(&mut inner);
                    drop(inner);
                    self.save_snapshot(true);
                }
            }
            EngineEvent::Error { id, message } => {
                tracing::warn!(%id, %message, "playback error");
                // The queued successor failed while the current track still plays: leave the
                // current one alone; the successor is retried (and skipped) when it is due.
                let successor = {
                    let mut inner = self.lock();
                    let successor = is_successor_error(
                        &id,
                        inner.state.track_id.as_ref(),
                        inner.pending_next.as_ref(),
                        inner.queue.peek_next(true).map(|i| &i.track_id),
                    );
                    if successor {
                        inner.pending_next = None;
                        inner.failed_next = Some(TrackId(id.clone()));
                    }
                    successor
                };
                if successor {
                    self.refresh_next();
                    return;
                }
                let listened = self.lock().listens.end(&id, None);
                self.log_listens(listened.into_iter().collect());
                (self.sink)(PlayerEvent::Error(trf!(
                    "Playback error: {message}",
                    "Помилка відтворення: {message}",
                    "Ошибка воспроизведения: {message}",
                )));
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
            (inner.state.track_id.clone(), inner.live_position())
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

    /// Adds finished plays to the listening log.
    fn log_listens(&self, done: Vec<Listen>) {
        for l in done {
            let row = l.clone();
            match self.db.write(move |t| stats::record(t, &row.track_id, row.started_at, row.listened_ms)) {
                Ok(()) => (self.sink)(PlayerEvent::Listened(l)),
                Err(e) => tracing::warn!(error = %e, "logging a play failed"),
            }
        }
    }

    /// Logs the plays in progress; called when the app quits.
    pub fn finish_listens(&self) {
        let done = self.lock().listens.finish();
        self.log_listens(done);
    }

    /// Persists what changed: the queue snapshot when the queue or mix did, the position and
    /// volume every [`POSITION_SAVE_EVERY`] while playing, soon after a seek or volume change,
    /// and always with `force` (pause, stop, exit).
    pub fn save_snapshot(&self, force: bool) {
        let (snapshot, pos) = {
            let mut inner = self.lock();
            let position_ms = inner.live_position();
            let snapshot = std::mem::take(&mut inner.queue_dirty).then(|| Snapshot {
                queue: inner.queue.clone(),
                position_ms,
                volume: inner.state.volume,
                mix: inner.mix.clone(),
            });
            let since = inner.last_position_save.elapsed();
            let playing = matches!(inner.state.status, Status::Playing | Status::Buffering);
            let due = force
                || snapshot.is_some()
                || (inner.position_dirty && since >= POSITION_SAVE_CHANGED)
                || (playing && since >= POSITION_SAVE_EVERY);
            let pos = due.then(|| {
                inner.last_position_save = Instant::now();
                inner.position_dirty = false;
                let current = inner.queue.current();
                PlaybackPos {
                    uid: current.map(|i| i.uid),
                    track_id: current.map(|i| i.track_id.clone()),
                    position_ms,
                    volume: inner.state.volume,
                }
            });
            (snapshot, pos)
        };
        if let Some(s) = snapshot
            && let Err(e) = settings::set(&self.db, SNAPSHOT_KEY, &s)
        {
            tracing::warn!(error = %e, "saving queue snapshot failed");
        }
        if let Some(p) = pos
            && let Err(e) = settings::set(&self.db, POSITION_KEY, &p)
        {
            tracing::warn!(error = %e, "saving playback position failed");
        }
    }

    // ---------- hand-off and release ----------

    /// The queue, live position, volume and mix, and whether it is playing: what a hand-off
    /// to the other app carries.
    pub fn snapshot(&self) -> (Snapshot, bool) {
        let inner = self.lock();
        let playing = matches!(inner.state.status, Status::Playing | Status::Buffering | Status::Loading);
        let snapshot = Snapshot {
            queue: inner.queue.clone(),
            position_ms: inner.live_position(),
            volume: inner.state.volume,
            mix: inner.mix.clone(),
        };
        (snapshot, playing)
    }

    /// Replaces the queue, mix, volume and position with a hand-off's and loads the current
    /// item there, playing when `play`. Without a current item the player goes idle.
    pub fn restore_snapshot(&self, snapshot: Snapshot, play: bool) {
        let (queue, position_ms, volume, mix) = restore(Some(snapshot), None);
        let current = queue.current().map(|i| i.track_id.clone());
        {
            let mut inner = self.lock();
            inner.queue = queue;
            inner.mix = mix;
            inner.skips = 0;
            inner.queue_dirty = true;
            inner.position_dirty = true;
            inner.state.volume = volume;
            inner.state.repeat = inner.queue.repeat();
            inner.state.shuffle = inner.queue.shuffle();
            inner.state.track_id = current.clone();
            inner.set_status(if current.is_some() { Status::Paused } else { Status::Idle });
            inner.set_position(position_ms);
        }
        self.engine.set_volume(volume);
        (self.sink)(PlayerEvent::QueueChanged);
        match current {
            Some(id) => {
                self.load_track(&id, position_ms, play);
            }
            None => {
                self.engine.stop();
                let mut inner = self.lock();
                self.emit_state(&mut inner);
            }
        }
        self.save_snapshot(true);
    }

    /// Releases the audio: pauses where it is, ends the listens in progress, saves the
    /// snapshot (`save_snapshot(true)`), then stops the engine thread, which closes the
    /// output device. The player ignores commands afterwards (nothing more is saved); a new
    /// [`Player::start`] on the same database continues from the saved position. Not for
    /// event handlers.
    pub fn shutdown(&self) {
        if self.is_shut_down() {
            return;
        }
        {
            let mut inner = self.lock();
            if matches!(inner.state.status, Status::Playing | Status::Buffering | Status::Loading) {
                inner.set_status(Status::Paused);
                inner.position_dirty = true;
            }
        }
        self.engine.pause();
        self.finish_listens();
        self.save_snapshot(true);
        self.engine.shutdown();
    }

    /// [`Player::shutdown`] ran (or the engine thread ended).
    pub fn is_shut_down(&self) -> bool {
        !self.engine.is_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(n: usize, current: usize) -> Queue {
        let mut q = Queue::new();
        q.set((0..n).map(|i| TrackId(format!("t{i}"))).collect(), current, QueueContext::None);
        q
    }

    fn snapshot(q: Queue, position_ms: u64) -> Snapshot {
        Snapshot { queue: q, position_ms, volume: 0.5, mix: None }
    }

    fn pos(q: &Queue, position_ms: u64) -> PlaybackPos {
        let current = q.current().unwrap();
        PlaybackPos {
            uid: Some(current.uid),
            track_id: Some(current.track_id.clone()),
            position_ms,
            volume: 0.3,
        }
    }

    #[test]
    fn successor_error_is_told_apart_from_the_current_one() {
        let (cur, next) = (TrackId::from("cur"), TrackId::from("nxt"));
        assert!(is_successor_error("nxt", Some(&cur), Some(&next), Some(&next)));
        assert!(is_successor_error("nxt", Some(&cur), None, Some(&next)));
        assert!(!is_successor_error("cur", Some(&cur), Some(&next), Some(&next)));
        // Repeat-one: the successor is the current track itself — a normal error.
        assert!(!is_successor_error("cur", Some(&cur), Some(&cur), Some(&cur)));
        assert!(!is_successor_error("old", Some(&cur), Some(&next), Some(&next)));
    }

    #[test]
    fn restores_the_position_of_the_same_item() {
        let q = queue(3, 1);
        let p = pos(&q, 95_000);
        let (q2, at, volume, _) = restore(Some(snapshot(q, 10_000)), Some(p));
        assert_eq!(q2.current().unwrap().track_id.as_str(), "t1");
        assert_eq!((at, volume), (95_000, 0.3));
    }

    #[test]
    fn ignores_a_position_of_another_item() {
        let mut q = queue(3, 2);
        let p = pos(&q, 95_000);
        // A new queue of the same tracks: new uids.
        q.set((0..3).map(|i| TrackId(format!("t{i}"))).collect(), 2, QueueContext::None);
        let (_, at, volume, _) = restore(Some(snapshot(q, 10_000)), Some(p));
        assert_eq!((at, volume), (10_000, 0.3));
    }

    #[test]
    fn reads_snapshots_from_before_playback_pos() {
        let json = serde_json::to_string(&snapshot(queue(2, 0), 42_000)).unwrap();
        let s: Snapshot = serde_json::from_str(&json).unwrap();
        let (q, at, volume, mix) = restore(Some(s), None);
        assert_eq!((q.items().len(), at, volume, mix.is_none()), (2, 42_000, 0.5, true));
        assert_eq!(restore(None, None).1, 0);
    }
}
