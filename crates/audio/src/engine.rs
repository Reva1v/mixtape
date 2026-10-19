//! The engine thread: owns the decoders and the output, turns commands into ring contents and
//! the device's consumed-frame counter into events.
//!
//! Tracks are pushed back to back into one ring (gapless); the [`Timeline`] remembers where
//! each starts so Started/Position/Ended describe what is audible, not what is decoded.
//!
//! A crossfade is done before the ring: when the decode head reaches the start of the overlap,
//! the successor becomes the head and a [`Fade`] keeps decoding the outgoing track into a
//! [`Mixer`] alongside it, so the ring only ever holds one already-mixed stream.
//!
//! Smart transitions ([`crate::smart`]) use the same path: the overlap starts on a planned
//! downbeat instead of a fixed time before the end, the successor is decoded through a
//! [`Varispeed`] into the mixer, and the mixer swaps the bass.
//!
//! Streamed sources ([`crate::SourceOpener::Sparse`]) never block this thread: their decoder
//! answers [`Error::Buffering`] instead of waiting for bytes, and the engine skips that decode
//! step (commands and events keep flowing) and tries again on the next turn. A load whose header
//! is not downloaded yet waits in `waiting`, a successor in `next_src`. While the head starves
//! and the ring is nearly empty, Buffering(true) is reported.
//!
//! Power: the thread sleeps as long as it can. While playing it refills the ring in bursts once
//! a third of it is free and sleeps until that is true again (≤ 10 wakeups/s); a starved stream
//! is polled every 50 ms. The device is opened on the first Play, not on a paused load; once
//! playback stops the stream is suspended (no device callbacks) and after a while closed, and
//! the thread blocks on its command channel until something arrives.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::analysis::TrackAnalysis;
use crate::crossfade::Mixer;
use crate::gain::volume_curve;
use crate::output::{Output, RING_MS};
use crate::pipeline::Pipeline;
use crate::smart::{self, BassSwap, SmartPlan, Varispeed};
use crate::timeline::{Handover, Timeline, Transition};
use crate::{Error, Result, Source};

/// Open the successor and ask for one this long (plus the crossfade) before the head ends.
const NEXT_LEAD_MS: u64 = 10_000;
const POSITION_EVERY: Duration = Duration::from_secs(1);
/// Bounds of the sleep between refills while playing.
const MIN_WAIT: Duration = Duration::from_millis(20);
const MAX_WAIT: Duration = Duration::from_millis(100);
/// Retry period of a streamed source waiting for its download.
const STARVED_WAIT: Duration = Duration::from_millis(50);
/// The same while paused (only until the stream is closed).
const PAUSED_POLL: Duration = Duration::from_millis(250);
/// Suspend the stream this long after playback stopped: the pause ramp and what the device
/// had buffered have played out by then.
const SUSPEND_AFTER: Duration = Duration::from_millis(50);
/// Close a stream that stayed suspended this long.
const CLOSE_AFTER: Duration = Duration::from_secs(30);
/// Packets decoded between command checks.
const PACKETS_PER_TURN: usize = 16;
/// A flush the callback does not acknowledge in time means the stream is stuck.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(250);
const REOPEN_EVERY: Duration = Duration::from_secs(1);
/// Underruns right after (re)priming are expected and not reported as buffering.
const PRIME_GRACE: Duration = Duration::from_millis(300);
const FALLBACK_RATE: u32 = 48_000;

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// The first sample of `id` reached the device (the start of the overlap when crossfading).
    Started {
        id: String,
        duration_ms: u64,
    },
    /// ~1 Hz while playing (and after loads and seeks), from frames the device actually
    /// consumed.
    Position {
        id: String,
        ms: u64,
    },
    StateChanged {
        playing: bool,
    },
    /// `id` is about 10 s plus the crossfade length (with smart transitions: plus its outro and
    /// the longest smart overlap) from its end and nothing is queued after it.
    NeedNext {
        id: String,
    },
    /// `id` finished (`completed`) or was replaced/stopped. A crossfaded-out track reports this
    /// when its last sample has played, after the successor's Started.
    Ended {
        id: String,
        played_ms: u64,
        completed: bool,
    },
    /// Playback waits for data: the device ran dry, or a streamed source's bytes are not
    /// downloaded yet (also right after a load or seek into a missing region); `false` once
    /// audio flows again.
    Buffering(bool),
    /// `id` could not be opened or decoded; the engine moves on to the queued next if any.
    Error {
        id: String,
        message: String,
    },
}

enum Command {
    Load {
        src: Source,
        start_ms: u64,
        play: bool,
    },
    CrossfadeTo {
        src: Source,
        ms: u32,
    },
    SetNext(Option<Source>),
    Crossfade(u32),
    Smart(bool),
    Analysis {
        id: String,
        analysis: Option<Arc<TrackAnalysis>>,
    },
    Play,
    Pause,
    Toggle,
    Seek(u64),
    Volume(f32),
    Gain {
        id: String,
        db: f32,
    },
    Normalize(bool),
    Stop,
}

/// Handle to the engine thread; dropping it stops playback and joins the thread.
pub struct Engine {
    tx: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// Spawns the engine thread. The output device is opened when something first plays and
    /// reopened after device errors, so this succeeds without a device.
    pub fn start() -> Result<(Engine, Receiver<EngineEvent>)> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (events, events_rx) = crossbeam_channel::unbounded();
        let thread = std::thread::Builder::new()
            .name("mixtape-audio".into())
            .spawn(move || State::new(events).run(rx))?;
        Ok((
            Engine {
                tx: Some(tx),
                thread: Some(thread),
            },
            events_rx,
        ))
    }

    /// Replaces whatever plays now (and the queued next) with `src` from `start_ms`.
    pub fn load(&self, src: Source, start_ms: u64, play: bool) {
        self.send(Command::Load {
            src,
            start_ms,
            play,
        });
    }

    /// Like `load(src, 0, true)`, but fades what is audible out and `src` in over `ms`
    /// (equal-power) instead of cutting. Paused or idle, it is exactly `load(src, 0, true)`.
    pub fn crossfade_to(&self, src: Source, ms: u32) {
        self.send(Command::CrossfadeTo { src, ms });
    }

    /// Queues the successor of the current track: gapless, or crossfaded when
    /// [`Engine::set_crossfade`] is on and `src.crossfade` allows it. `None` clears it and
    /// means "nothing follows": NeedNext is not sent again until another track becomes the
    /// decode head.
    pub fn set_next(&self, src: Option<Source>) {
        self.send(Command::SetNext(src));
    }

    /// Length of automatic crossfades between tracks; 0 (the default) is gapless. The overlap
    /// is clamped to half of either track.
    pub fn set_crossfade(&self, ms: u32) {
        self.send(Command::Crossfade(ms));
    }

    /// Smart transitions on or off. On, an automatic transition between two analysed tracks
    /// that may crossfade is planned from their beats, keys and structure; otherwise (and
    /// when off) [`Engine::set_crossfade`] applies as before.
    pub fn set_smart(&self, on: bool) {
        self.send(Command::Smart(on));
    }

    /// Hands over the analysis of every source with this id (playing, queued): it finished
    /// after the source was built.
    pub fn set_analysis(&self, id: impl Into<String>, analysis: Option<Arc<TrackAnalysis>>) {
        self.send(Command::Analysis {
            id: id.into(),
            analysis,
        });
    }

    pub fn play(&self) {
        self.send(Command::Play);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn toggle(&self) {
        self.send(Command::Toggle);
    }

    /// Seeks within the audible track. During a crossfade that is the incoming track; the
    /// outgoing one is dropped.
    pub fn seek(&self, ms: u64) {
        self.send(Command::Seek(ms));
    }

    /// User volume 0…1 (perceptual curve applied here).
    pub fn set_volume(&self, v: f32) {
        self.send(Command::Volume(v));
    }

    /// Replaces the normalisation gain of every source with this id (playing, fading out,
    /// queued): its loudness became known after it was handed over. A playing track glides
    /// to it over ~50 ms; what is already in the ring (≤ 300 ms) keeps the old gain.
    pub fn set_gain(&self, id: impl Into<String>, gain_db: f32) {
        self.send(Command::Gain {
            id: id.into(),
            db: gain_db,
        });
    }

    /// Loudness normalisation on (the default) or off: off plays every source at unity
    /// instead of its [`Source::gain_db`]. Playing tracks glide to the new gain.
    pub fn set_normalization(&self, on: bool) {
        self.send(Command::Normalize(on));
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The outgoing track of a crossfade, decoded under the head into the mixer.
struct Fade {
    /// Its pipeline stays in `slots` while it is the audible track, then moves to `a`.
    a_seq: u64,
    a: Option<Pipeline>,
    mix: Mixer,
    /// Smart transition at another tempo: the head is decoded into this, not the mixer.
    stretch: Option<Varispeed>,
}

/// An automatic transition the head is heading for (output frames of the head).
struct AutoFade {
    start: u64,
    len: u64,
    smart: Option<SmartPlan>,
}

/// Opens the output device: `(volume, paused)`. Tests swap in a fake.
type Opener = Box<dyn FnMut(f32, bool) -> Result<Output> + Send>;

struct State {
    events: Sender<EngineEvent>,
    open_output: Opener,
    out: Option<Output>,
    reopen_at: Instant,
    /// Rate of the last opened device: pipelines opened without a device use it, so they
    /// rarely need a new resampler when it opens.
    dev_rate: u32,
    /// When playback last stopped; `None` while playing.
    quiet_since: Option<Instant>,
    /// Decoding until the ring is full (see [`State::pump`]).
    refilling: bool,
    /// The last turn stopped decoding at [`PACKETS_PER_TURN`] with more to do.
    busy: bool,
    timeline: Timeline,
    /// The audible track first, the decode head last.
    slots: VecDeque<Pipeline>,
    /// Successor opened ahead of time.
    next: Option<Pipeline>,
    next_src: Option<Source>,
    /// A crossfade into the head being decoded.
    fade: Option<Fade>,
    crossfade_ms: u32,
    /// Smart transitions between analysed tracks.
    smart: bool,
    /// Decoded audio waiting for room in the ring.
    pending: Vec<f32>,
    pending_at: usize,
    /// Frames pushed into the current ring.
    written: u64,
    head_done: bool,
    playing: bool,
    volume: f32,
    /// Sources play at their `gain_db` (else at unity).
    normalize: bool,
    seq: u64,
    need_next_sent: bool,
    last_position: Instant,
    primed_at: Instant,
    underruns_seen: u64,
    buffering: bool,
    /// A loaded streamed source (and its start) whose header is not downloaded yet.
    waiting: Option<(Source, u64)>,
    /// The last decode step had to wait for a download.
    starved: bool,
    transitions: Vec<Transition>,
    /// Where the user last sought to: `(seq, out_pos)` of that head. A seek into the tail
    /// that would already be in the automatic transition plays the track to its end instead.
    seek_mark: Option<(u64, u64)>,
}

impl State {
    fn new(events: Sender<EngineEvent>) -> Self {
        let now = Instant::now();
        let open_output: Opener = if cfg!(test) {
            // Unit tests never touch the real device.
            Box::new(|_, _| Err(Error::Output("no device in tests".into())))
        } else {
            Box::new(Output::open)
        };
        Self {
            events,
            open_output,
            out: None,
            reopen_at: now,
            dev_rate: FALLBACK_RATE,
            quiet_since: Some(now),
            refilling: false,
            busy: false,
            timeline: Timeline::new(FALLBACK_RATE),
            slots: VecDeque::new(),
            next: None,
            next_src: None,
            fade: None,
            crossfade_ms: 0,
            smart: false,
            pending: Vec::new(),
            pending_at: 0,
            written: 0,
            head_done: false,
            playing: false,
            volume: 1.0,
            normalize: true,
            seq: 0,
            need_next_sent: false,
            last_position: now,
            primed_at: now,
            underruns_seen: 0,
            buffering: false,
            waiting: None,
            starved: false,
            transitions: Vec::new(),
            seek_mark: None,
        }
    }

    fn run(mut self, rx: Receiver<Command>) {
        loop {
            let got = match self.next_wait() {
                Some(wait) => rx.recv_timeout(wait),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match got {
                Ok(cmd) => {
                    self.handle(cmd);
                    while let Ok(cmd) = rx.try_recv() {
                        self.handle(cmd);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.turn();
        }
    }

    /// One pass after the commands: device upkeep, decoding, events.
    fn turn(&mut self) {
        self.open_waiting();
        self.check_output();
        self.busy = false;
        let mut steps = 0;
        while self.pump() {
            steps += 1;
            if steps == PACKETS_PER_TURN {
                self.busy = true;
                break;
            }
        }
        self.tick();
        self.idle_output();
    }

    /// How long the thread may sleep before the next turn; `None` until a command arrives.
    fn next_wait(&self) -> Option<Duration> {
        if self.busy {
            return Some(Duration::ZERO);
        }
        if self.playing {
            let Some(out) = &self.out else {
                if self.starved {
                    return Some(STARVED_WAIT);
                }
                // The device failed to open: retry when allowed.
                let retry = self.reopen_at.saturating_duration_since(Instant::now());
                return Some(retry.clamp(MIN_WAIT, REOPEN_EVERY));
            };
            if self.starved {
                return Some(STARVED_WAIT);
            }
            // Sleep until a third of the ring is free again (the refill threshold), or until it
            // runs dry when nothing more will be decoded.
            let (cap, filled) = out.fill();
            let more = !self.head_done || self.next.is_some() || self.next_src.is_some();
            let target = if more { cap - cap / 3 } else { 0 };
            let ms = filled.saturating_sub(target) * 1000 / out.rate.max(1) as u64;
            return Some(Duration::from_millis(ms).clamp(MIN_WAIT, MAX_WAIT));
        }
        let out = self.out.as_ref()?;
        let starving = self.starved || self.waiting.is_some();
        match out.suspended_for() {
            None => {
                let quiet = self.quiet_since.map_or(SUSPEND_AFTER, |t| t.elapsed());
                Some(SUSPEND_AFTER.saturating_sub(quiet).max(Duration::from_millis(1)))
            }
            // Paused on a stream still downloading: keep filling the ring for the resume, slowly.
            Some(_) if starving => Some(PAUSED_POLL),
            Some(t) => Some(CLOSE_AFTER.saturating_sub(t).max(Duration::from_millis(1))),
        }
    }

    fn emit(&self, e: EngineEvent) {
        let _ = self.events.send(e);
    }

    fn emit_transition(&self, t: Transition) {
        self.emit(match t {
            Transition::Started {
                id, duration_ms, ..
            } => EngineEvent::Started { id, duration_ms },
            Transition::Ended {
                id,
                played_ms,
                completed,
            } => EngineEvent::Ended {
                id,
                played_ms,
                completed,
            },
        });
    }

    fn emit_error(&self, id: String, e: impl ToString) {
        self.emit(EngineEvent::Error {
            id,
            message: e.to_string(),
        });
    }

    fn rate(&self) -> u32 {
        self.out.as_ref().map_or(self.dev_rate, |o| o.rate)
    }

    fn consumed(&self) -> u64 {
        self.out
            .as_ref()
            .map_or(self.written, |o| o.shared.consumed.load(Ordering::Acquire))
    }

    /// Ring frame at which the next decoded frame will land.
    fn queued_end(&self) -> u64 {
        self.written + ((self.pending.len() - self.pending_at) / 2) as u64
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Load {
                src,
                start_ms,
                play,
            } => self.load(src, start_ms, play),
            Command::CrossfadeTo { src, ms } => self.crossfade_to(src, ms),
            Command::SetNext(src) => self.set_next(src),
            Command::Crossfade(ms) => {
                self.crossfade_ms = ms;
                self.measure_all();
            }
            Command::Smart(on) => {
                self.smart = on;
                self.measure_all();
            }
            Command::Analysis { id, analysis } => self.set_analysis(&id, analysis),
            Command::Play => self.play(),
            Command::Pause => self.set_playing(false),
            Command::Toggle if self.playing => self.set_playing(false),
            Command::Toggle => self.play(),
            Command::Seek(ms) => self.seek(ms),
            Command::Volume(v) => {
                self.volume = volume_curve(v);
                if let Some(o) = &self.out {
                    o.shared
                        .volume
                        .store(self.volume.to_bits(), Ordering::Relaxed);
                }
            }
            Command::Gain { id, db } => self.set_gain(&id, db),
            Command::Normalize(on) => {
                self.normalize = on;
                self.pipelines_mut()
                    .for_each(|p| p.set_normalize(on, true));
            }
            Command::Stop => self.stop(),
        }
    }

    /// Every open pipeline: the audible and decoding ones, a crossfaded-out one, the successor.
    fn pipelines_mut(&mut self) -> impl Iterator<Item = &mut Pipeline> {
        self.slots
            .iter_mut()
            .chain(&mut self.next)
            .chain(self.fade.as_mut().and_then(|f| f.a.as_mut()))
    }

    /// Transitions overlap, so track lengths are needed up front.
    fn fades(&self) -> bool {
        self.crossfade_ms > 0 || self.smart
    }

    fn measure_all(&mut self) {
        if self.fades() {
            self.slots
                .iter_mut()
                .chain(&mut self.next)
                .for_each(Pipeline::measure);
        }
    }

    fn set_analysis(&mut self, id: &str, analysis: Option<Arc<TrackAnalysis>>) {
        self.pipelines_mut()
            .filter(|p| p.id == id)
            .for_each(|p| p.analysis = analysis.clone());
        let waiting = self.waiting.as_mut().map(|(s, _)| s);
        for src in waiting.into_iter().chain(&mut self.next_src) {
            if src.id == id {
                src.analysis = analysis.clone();
            }
        }
    }

    fn set_gain(&mut self, id: &str, db: f32) {
        self.pipelines_mut()
            .filter(|p| p.id == id)
            .for_each(|p| p.set_gain_db(db, true));
        let waiting = self.waiting.as_mut().map(|(s, _)| s);
        for src in waiting.into_iter().chain(&mut self.next_src) {
            if src.id == id {
                src.gain_db = db;
            }
        }
    }

    /// Opens `src` at the current rate, with the normalisation setting applied.
    fn open_pipeline(&self, src: Source) -> Result<Pipeline> {
        let mut p = Pipeline::open(src, self.rate())?;
        p.set_normalize(self.normalize, false);
        Ok(p)
    }

    fn load(&mut self, src: Source, start_ms: u64, play: bool) {
        // A paused load (e.g. restoring the last track at startup) leaves the device closed
        // until the first Play.
        if play {
            self.ensure_output();
        }
        self.reset();
        let id = src.id.clone();
        if !src.open.ready() {
            return self.wait_for(src, start_ms, play);
        }
        match self.open_pipeline(src.clone()) {
            Ok(p) => self.start_fresh(p, start_ms, play),
            Err(Error::Buffering) => self.wait_for(src, start_ms, play),
            Err(e) => {
                self.emit_error(id, e);
                self.set_playing(false);
            }
        }
    }

    /// Holds a streamed source until its header is downloaded (see [`State::open_waiting`]).
    fn wait_for(&mut self, src: Source, start_ms: u64, play: bool) {
        let id = src.id.clone();
        self.waiting = Some((src, start_ms));
        self.starved = true;
        self.emit(EngineEvent::Position { id, ms: start_ms });
        self.set_playing(play);
    }

    /// Opens the waiting source once its header is there.
    fn open_waiting(&mut self) {
        let Some((src, start_ms)) = self.waiting.take_if(|(s, _)| s.open.ready()) else {
            return;
        };
        let id = src.id.clone();
        match self.open_pipeline(src.clone()) {
            Ok(p) => self.start_fresh(p, start_ms, self.playing),
            Err(Error::Buffering) => self.waiting = Some((src, start_ms)),
            Err(e) => {
                self.emit_error(id, e);
                self.set_playing(false);
            }
        }
    }

    /// Plays `p` from `start_ms` into an empty ring.
    fn start_fresh(&mut self, mut p: Pipeline, start_ms: u64, play: bool) {
        let id = p.id.clone();
        if start_ms > 0
            && let Err(e) = p.seek(start_ms)
        {
            self.emit_error(id.clone(), e);
        }
        p.fade_in();
        self.start_head(p, start_ms, false);
        self.emit(EngineEvent::Position { id, ms: start_ms });
        self.primed_at = Instant::now();
        self.set_playing(play);
    }

    /// Manual crossfade. When the decode head is the audible track the fade starts where
    /// decoding is (at most a ring length ahead of the device), seamlessly. Otherwise (a
    /// successor or a fade already in the ring) the audible track is re-primed at the device
    /// position first, which costs the usual 10 ms flush dip before the fade.
    fn crossfade_to(&mut self, src: Source, ms: u32) {
        let cut = ms == 0 || !self.playing || self.slots.is_empty() || self.timeline.ended();
        // A stream that is not downloaded yet has nothing to fade in: cut and buffer.
        if cut || !src.open.ready() {
            return self.load(src, 0, true);
        }
        let id = src.id.clone();
        let b = match self.open_pipeline(src.clone()) {
            Ok(p) => p,
            Err(Error::Buffering) => return self.load(src, 0, true),
            Err(e) => {
                self.reset();
                self.emit_error(id, e);
                self.set_playing(false);
                return;
            }
        };
        let direct = self.fade.is_none() && self.slots.len() == 1 && !self.head_done;
        if !direct && !self.reprime_here() {
            self.reset();
            return self.start_fresh(b, 0, true);
        }
        self.next = None;
        self.next_src = None;
        let mut len = self.rate() as u64 * ms as u64 / 1000;
        if let Some(lb) = b.out_len() {
            len = len.min(lb / 2);
        }
        self.begin_fade(b, len, Vec::new(), false, 0, None);
    }

    /// Makes `p` the decode head; its first frame lands at the current end of the queue.
    fn start_head(&mut self, mut p: Pipeline, start_ms: u64, started: bool) {
        self.seq += 1;
        p.seq = self.seq;
        if self.fades() {
            // Crossfades and an early NeedNext need the length up front.
            p.measure();
        }
        let duration = p.duration_ms().unwrap_or(0);
        self.timeline
            .push_track(self.queued_end(), p.seq, &p.id, start_ms, duration, started);
        self.slots.push_back(p);
        self.head_done = false;
        self.need_next_sent = false;
    }

    /// Starts mixing `b` in under the head over `len` frames: `b` becomes the head, the old
    /// head keeps decoding into the fade. `a_tail` is old-head audio already decoded past the
    /// start of the overlap; `b_ms` is where `b`'s positions count from. A smart plan adds
    /// the bass swap and the tempo match.
    fn begin_fade(
        &mut self,
        b: Pipeline,
        len: u64,
        a_tail: Vec<f32>,
        completed: bool,
        b_ms: u64,
        smart: Option<SmartPlan>,
    ) {
        let Some(a_seq) = self.slots.back().map(|p| p.seq) else {
            return;
        };
        let mut mix = Mixer::new(len);
        mix.a.buf = a_tail;
        mix.a.ended = self.head_done;
        let mut stretch = None;
        if let Some(plan) = smart {
            mix.swap = Some(BassSwap::new(&plan, self.rate()));
            if plan.speed != 1.0 {
                stretch = Some(Varispeed::new(plan));
            }
        }
        self.fade = Some(Fade {
            a_seq,
            a: None,
            mix,
            stretch,
        });
        self.start_head(b, b_ms, false);
        self.timeline.set_handover(Handover::Overlap {
            frames: len.max(1),
            completed,
        });
    }

    fn set_next(&mut self, src: Option<Source>) {
        let same = |p: &Pipeline| src.as_ref().is_some_and(|s| s.id == p.id);
        if self.slots.len() > 1 {
            // The old successor is already in the ring: keep it if unchanged, else re-prime.
            if self.slots.back().is_some_and(same) {
                return;
            }
            self.reprime_here();
            self.next = None;
        }
        if self.next.as_ref().is_some_and(same) {
            return;
        }
        self.next = None;
        // `None` means "nothing follows": do not ask again until the head changes.
        self.need_next_sent = true;
        self.next_src = src;
    }

    /// Resumes; after the last track ended, plays it again from the start.
    fn play(&mut self) {
        if self.timeline.ended() && !self.slots.is_empty() {
            self.seek(0);
        }
        self.set_playing(true);
    }

    fn set_playing(&mut self, playing: bool) {
        let playing = playing && (!self.slots.is_empty() || self.waiting.is_some());
        if let Some(o) = &mut self.out {
            o.shared.paused.store(!playing, Ordering::Relaxed);
            if playing && let Err(e) = o.resume() {
                tracing::warn!("cannot resume audio output: {e}");
                o.shared.failed.store(true, Ordering::Release);
            }
        }
        // Without a device, the turn after this opens one (see `check_output`).
        if playing {
            self.quiet_since = None;
        } else if self.quiet_since.is_none() {
            self.quiet_since = Some(Instant::now());
        }
        if self.playing && !playing {
            // Positions are sparse: say exactly where it paused.
            if let Some((id, ms)) = self.timeline.position(self.consumed()) {
                let id = id.to_string();
                self.emit(EngineEvent::Position { id, ms });
            }
        }
        if playing != self.playing {
            self.playing = playing;
            self.emit(EngineEvent::StateChanged { playing });
        }
    }

    fn seek(&mut self, ms: u64) {
        if let Some((src, start_ms)) = &mut self.waiting {
            *start_ms = ms;
            let id = src.id.clone();
            return self.emit(EngineEvent::Position { id, ms });
        }
        let Some(id) = self.slots.front().map(|p| p.id.clone()) else {
            return;
        };
        let started = self.timeline.started();
        self.flush();
        self.reprime(ms, started);
        self.mark_seek();
        self.primed_at = Instant::now();
        self.emit(EngineEvent::Position { id, ms });
    }

    fn stop(&mut self) {
        self.reset();
        self.set_playing(false);
    }

    /// Ends what is audible and forgets every track.
    fn reset(&mut self) {
        self.interrupt();
        self.flush();
        self.slots.clear();
        self.next = None;
        self.next_src = None;
        self.fade = None;
        self.timeline.clear(self.rate());
        self.head_done = false;
        self.waiting = None;
        self.starved = false;
    }

    /// Reports the audible track (and one fading out under it) as cut short.
    fn interrupt(&mut self) {
        let mut ended = Vec::new();
        self.timeline.interrupt(self.consumed(), &mut ended);
        for t in ended {
            self.emit_transition(t);
        }
    }

    /// Drops everything queued for the device (with a short fade) and waits until it is gone.
    fn flush(&mut self) {
        self.pending.clear();
        self.pending_at = 0;
        let Some(out) = &mut self.out else {
            return;
        };
        let (_, filled) = out.fill();
        if filled == 0 {
            // Nothing queued for the device, so nothing to fade or drop (and a suspended
            // stream can stay suspended).
            return;
        }
        // A suspended stream has no callback to drop the ring and acknowledge. Paused, the
        // callback only discards and plays silence; `idle_output` suspends it again.
        if let Err(e) = out.resume() {
            tracing::warn!("cannot resume audio output, reopening the device: {e}");
            self.out = None;
            self.written = 0;
            return;
        }
        let shared = out.shared.clone();
        let generation = shared.flush.load(Ordering::Relaxed) + 1;
        shared.flush.store(generation, Ordering::Release);
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        while shared.flushed.load(Ordering::Acquire) != generation {
            if Instant::now() > deadline {
                tracing::warn!("audio callback stalled, reopening the device");
                self.out = None;
                self.written = 0;
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Flushes and restarts the audible track where the device is now. Returns whether there
    /// was an audible track.
    fn reprime_here(&mut self) -> bool {
        let consumed = self.consumed();
        let Some(ms) = self.timeline.position(consumed).map(|(_, ms)| ms) else {
            return false;
        };
        let started = self.timeline.started();
        // Restarting the track the user sought in keeps it out of the transition.
        let marked = self
            .seek_mark
            .is_some_and(|(seq, _)| self.slots.front().is_some_and(|p| p.seq == seq));
        self.flush();
        self.reprime(ms, started);
        if marked {
            self.mark_seek();
        }
        true
    }

    /// Remembers that the user sought to where the head is now.
    fn mark_seek(&mut self) {
        self.seek_mark = self.slots.back().map(|p| (p.seq, p.out_pos()));
    }

    /// Restarts decoding of the audible track at `ms` into an empty ring. A successor that was
    /// already decoding goes back to being the queued next; a crossfade is cancelled (a track
    /// fading out is dropped).
    fn reprime(&mut self, ms: u64, started: bool) {
        if let Some(t) = self.timeline.take_outgoing(self.consumed()) {
            self.emit_transition(t);
        }
        self.fade = None;
        self.pending.clear();
        self.pending_at = 0;
        let rate = self.rate();
        self.timeline.clear(rate);
        while self.slots.len() > 1 {
            let mut p = self.slots.pop_back().expect("len > 1");
            if self.slots.len() == 1 {
                match p.seek(0) {
                    Ok(()) => self.next = Some(p),
                    Err(e) => self.emit_error(p.id, e),
                }
            }
        }
        let Some(mut p) = self.slots.pop_front() else {
            return;
        };
        let res = p.set_out_rate(rate).and_then(|()| p.seek(ms));
        if let Err(e) = res {
            self.emit_error(p.id.clone(), e);
        }
        if let Some(mut n) = self.next.take() {
            match n.set_out_rate(rate) {
                Ok(()) => self.next = Some(n),
                Err(e) => self.emit_error(n.id, e),
            }
        }
        let asked = self.need_next_sent;
        self.start_head(p, ms, started);
        self.need_next_sent = asked;
    }

    fn ensure_output(&mut self) {
        if self.out.is_some() || Instant::now() < self.reopen_at {
            return;
        }
        match (self.open_output)(self.volume, !self.playing) {
            Ok(out) => {
                self.dev_rate = out.rate;
                self.out = Some(out);
                self.written = 0;
                self.underruns_seen = 0;
                self.refilling = false;
            }
            Err(e) => {
                tracing::warn!("cannot open audio output: {e}");
                self.reopen_at = Instant::now() + REOPEN_EVERY;
            }
        }
    }

    /// Rebuilds a failed stream (device error, default device changed) and resumes the audible
    /// track where the old device stopped. Opens the device when something should play and
    /// there is none (the first Play, after a failure, after an idle close).
    fn check_output(&mut self) {
        let failed = self
            .out
            .as_ref()
            .is_some_and(|o| o.shared.failed.load(Ordering::Acquire));
        if failed {
            self.close_output();
            self.reopen_at = Instant::now();
        }
        if self.out.is_some() || self.slots.is_empty() || !self.playing {
            return;
        }
        self.ensure_output();
        if self.out.is_some() {
            // Without a device nothing was consumed, so the position is where decoding restarts.
            let resume = self.timeline.position(0).map(|(_, ms)| ms);
            let started = self.timeline.started();
            match resume {
                Some(ms) => self.reprime(ms, started),
                None => self.timeline.clear(self.rate()),
            }
            self.primed_at = Instant::now();
        }
    }

    /// Drops the device, keeping the audible track (re-primed at its audible position) for
    /// the next one.
    fn close_output(&mut self) {
        if self.out.is_none() {
            return;
        }
        // The audible position is only known through the old ring.
        let resume = self.timeline.position(self.consumed()).map(|(_, ms)| ms);
        let started = self.timeline.started();
        self.out = None;
        self.written = 0;
        self.refilling = false;
        if let Some(ms) = resume {
            self.reprime(ms, started);
        }
    }

    /// While nothing plays: suspends the stream once the pause ramp has played out, and
    /// closes it after it stayed suspended for a while.
    fn idle_output(&mut self) {
        if self.playing {
            return;
        }
        let Some(out) = &mut self.out else {
            return;
        };
        match out.suspended_for() {
            None if self.quiet_since.is_none_or(|t| t.elapsed() >= SUSPEND_AFTER) => {
                out.suspend()
            }
            Some(t) if t >= CLOSE_AFTER => {
                tracing::debug!("closing the idle audio output");
                self.close_output();
            }
            _ => {}
        }
    }

    /// Moves one step of audio towards the ring. Returns whether anything happened.
    ///
    /// Decoding runs in bursts: it starts once a third of the ring is free and goes on until
    /// the ring is full (or the source has nothing yet), so the thread can sleep in between.
    fn pump(&mut self) -> bool {
        let Some(out) = &mut self.out else {
            return false;
        };
        let free = out.ring.slots();
        if !self.refilling && free >= out.ring.buffer().capacity() / 3 {
            self.refilling = true;
        }
        if !self.refilling || free == 0 {
            self.refilling = false;
            return false;
        }
        if self.pending_at < self.pending.len() {
            let (pushed, rest) = out
                .ring
                .push_partial_slice(&self.pending[self.pending_at..]);
            let n = pushed.len();
            let done = rest.is_empty();
            self.pending_at += n;
            self.written += (n / 2) as u64;
            if done {
                self.pending.clear();
                self.pending_at = 0;
            } else {
                // Full: wait for the device to drain it.
                self.refilling = false;
            }
            return n > 0;
        }
        let progressed = self.produce();
        if !progressed {
            self.refilling = false;
        }
        progressed
    }

    /// Decodes one step into `pending`, which must be empty. Returns whether anything happened.
    fn produce(&mut self) -> bool {
        if self.slots.is_empty() {
            return false;
        }
        if self.fade.is_some() {
            return self.fade_step();
        }
        if self.head_done {
            return self.advance_head();
        }
        let plan = self.auto_fade();
        let from = self.pending.len();
        let Some(head) = self.slots.back_mut() else {
            return false;
        };
        let before = head.out_pos();
        match head.fill(&mut self.pending) {
            Ok(more) => {
                self.head_done = !more;
                self.starved = false;
            }
            Err(Error::Buffering) => {
                self.starved = true;
                return false;
            }
            Err(e) => {
                let (id, message) = (head.id.clone(), e.to_string());
                tracing::warn!(%id, "decode failed: {message}");
                self.emit(EngineEvent::Error { id, message });
                self.head_done = true;
            }
        }
        // This chunk crossed into the overlap (or the successor arrived late, after its start:
        // then the overlap is what is left of the head).
        if let Some(fade) = plan
            && before < fade.start + fade.len
        {
            let start = fade.start.max(before);
            let after = before + ((self.pending.len() - from) / 2) as u64;
            if after > start {
                let tail = self.pending.split_off(from + 2 * (start - before) as usize);
                let b = self.next.take().expect("planned with a successor");
                match fade.smart {
                    Some(plan) => self.begin_smart(b, plan, tail),
                    None => self.begin_fade(b, fade.start + fade.len - start, tail, true, 0, None),
                }
            }
        }
        true
    }

    /// The transition into the opened successor, when they overlap: a smart one while it can
    /// still start where planned, else the plain crossfade.
    fn auto_fade(&self) -> Option<AutoFade> {
        let (a, b) = (self.slots.back()?, self.next.as_ref()?);
        if !b.crossfade {
            return None;
        }
        // Sought past where the transition starts: play to the end, then gapless.
        let sought = self
            .seek_mark
            .and_then(|(seq, at)| (seq == a.seq).then_some(at));
        if let Some(plan) = self.smart_plan(a, b)
            && a.out_pos() <= plan.start
        {
            return Some(AutoFade {
                start: plan.start,
                len: plan.len,
                smart: Some(plan),
            });
        }
        if self.crossfade_ms == 0 {
            return None;
        }
        let (la, lb) = (a.out_len()?, b.out_len()?);
        let len = (self.rate() as u64 * self.crossfade_ms as u64 / 1000)
            .min(la / 2)
            .min(lb / 2);
        (len > 0 && sought.is_none_or(|at| at < la - len)).then_some(AutoFade {
            start: la - len,
            len,
            smart: None,
        })
    }

    fn smart_plan(&self, a: &Pipeline, b: &Pipeline) -> Option<SmartPlan> {
        if !self.smart {
            return None;
        }
        let (aa, ba) = (a.analysis.as_deref()?, b.analysis.as_deref()?);
        smart::plan(aa, ba, a.duration_ms()?, b.duration_ms()?, self.rate())
    }

    /// Starts a planned smart transition: `b` from its first beat, positions corrected for
    /// the tempo change of the overlap.
    fn begin_smart(&mut self, mut b: Pipeline, plan: SmartPlan, a_tail: Vec<f32>) {
        if plan.b_start_ms > 0
            && let Err(e) = b.seek(plan.b_start_ms)
        {
            self.emit_error(b.id.clone(), e);
        }
        let b_ms = plan.timeline_start_ms(self.rate());
        self.begin_fade(b, plan.len, a_tail, true, b_ms, Some(plan));
    }

    /// Feeds the mixer from whichever side it waits for, or mixes. Returns whether anything
    /// happened (not while a side waits for its download).
    fn fade_step(&mut self) -> bool {
        let Some(fade) = &mut self.fade else {
            return false;
        };
        let mut failed = None;
        if fade.mix.needs_a() {
            let a = match &mut fade.a {
                Some(a) => Some(a),
                None => self.slots.iter_mut().find(|p| p.seq == fade.a_seq),
            };
            let res = match a {
                Some(a) => a.fill(&mut fade.mix.a.buf).map_err(|e| (a.id.clone(), e)),
                None => Ok(false),
            };
            fade.mix.a.ended = match res {
                Ok(more) => !more,
                Err((_, Error::Buffering)) => {
                    self.starved = true;
                    return false;
                }
                Err(f) => {
                    failed = Some(f);
                    true
                }
            };
        } else if fade.mix.needs_b() {
            let Some(head) = self.slots.back_mut() else {
                return false;
            };
            match &mut fade.stretch {
                Some(vs) => {
                    if vs.wants_input() {
                        vs.input_ended = match head.fill(&mut vs.raw) {
                            Ok(more) => !more,
                            Err(Error::Buffering) => {
                                self.starved = true;
                                return false;
                            }
                            Err(e) => {
                                failed = Some((head.id.clone(), e));
                                true
                            }
                        };
                        self.head_done = vs.input_ended;
                    }
                    vs.produce(&mut fade.mix.b.buf);
                    fade.mix.b.ended = vs.dry();
                }
                None => {
                    fade.mix.b.ended = match head.fill(&mut fade.mix.b.buf) {
                        Ok(more) => !more,
                        Err(Error::Buffering) => {
                            self.starved = true;
                            return false;
                        }
                        Err(e) => {
                            failed = Some((head.id.clone(), e));
                            true
                        }
                    };
                    self.head_done = fade.mix.b.ended;
                }
            }
        } else {
            fade.mix.mix(&mut self.pending);
            if fade.mix.finished() {
                let fade = self.fade.take().expect("fading");
                self.pending.extend_from_slice(&fade.mix.into_rest());
                if let Some(vs) = fade.stretch {
                    self.pending.extend_from_slice(&vs.into_rest());
                }
            }
        }
        self.starved = false;
        if let Some((id, e)) = failed {
            tracing::warn!(%id, "decode failed: {e}");
            self.emit_error(id, e);
        }
        true
    }

    /// The head is fully in the ring: continue with the successor, or mark the end.
    fn advance_head(&mut self) -> bool {
        if self.next.is_none() {
            self.open_next();
        }
        match self.next.take() {
            Some(p) => {
                self.start_head(p, 0, false);
                true
            }
            // A streamed successor whose header is still downloading.
            None if self.next_src.is_some() => {
                self.starved = true;
                false
            }
            None => {
                self.timeline.push_end(self.written);
                false
            }
        }
    }

    /// Opens the queued successor, unless it is a stream whose header is not there yet.
    fn open_next(&mut self) {
        let Some(src) = self.next_src.take_if(|s| s.open.ready()) else {
            return;
        };
        let id = src.id.clone();
        match self.open_pipeline(src.clone()) {
            Ok(mut p) => {
                if self.fades() && p.crossfade {
                    p.measure();
                }
                self.next = Some(p);
            }
            Err(Error::Buffering) => self.next_src = Some(src),
            Err(e) => {
                self.emit_error(id, e);
                self.need_next_sent = false;
            }
        }
    }

    fn tick(&mut self) {
        let consumed = self.consumed();
        self.timeline.advance(consumed, &mut self.transitions);
        for t in std::mem::take(&mut self.transitions) {
            if let Transition::Started { seq, .. } = &t {
                while self.slots.len() > 1 && self.slots.front().is_some_and(|p| p.seq != *seq) {
                    let p = self.slots.pop_front().expect("len > 1");
                    // A track fading out is no longer audible-first but still feeds the mixer.
                    if let Some(f) = &mut self.fade
                        && f.a_seq == p.seq
                    {
                        f.a = Some(p);
                    }
                }
            }
            self.emit_transition(t);
        }
        if self.timeline.ended() && self.playing {
            self.set_playing(false);
        }

        let lead = NEXT_LEAD_MS + self.crossfade_ms as u64;
        let near_end = self.slots.back().is_some_and(|head| {
            self.head_done
                || head.duration_ms().is_some_and(|d| {
                    // A smart transition may start in the outro, well before the end.
                    let smart = match &head.analysis {
                        Some(a) if self.smart => NEXT_LEAD_MS + smart::lead_ms(a, d),
                        _ => 0,
                    };
                    d.saturating_sub(head.position_ms()) <= lead.max(smart)
                })
        });
        if near_end && self.next.is_none() {
            self.open_next();
            if self.next.is_none() && self.next_src.is_none() && !self.need_next_sent {
                self.need_next_sent = true;
                let id = self.slots.back().map(|p| p.id.clone()).unwrap_or_default();
                self.emit(EngineEvent::NeedNext { id });
            }
        }

        if self.playing && self.last_position.elapsed() >= POSITION_EVERY {
            self.last_position = Instant::now();
            if let Some((id, ms)) = self.timeline.position(consumed) {
                self.emit(EngineEvent::Position {
                    id: id.to_string(),
                    ms,
                });
            }
        }
        self.check_buffering();
    }

    /// Buffering(true) on an underrun of a slow source, or as soon as a starved stream has let
    /// the queue run nearly dry (at once after a load or seek); `false` once refilled.
    fn check_buffering(&mut self) {
        let mut underrun = false;
        // Without a device (tests) there is no ring to refill.
        let mut refilled = !self.starved;
        if let Some(out) = &self.out {
            let underruns = out.shared.underruns.load(Ordering::Relaxed);
            underrun = underruns > self.underruns_seen;
            self.underruns_seen = underruns;
            let filled = out.ring.buffer().capacity() - out.ring.slots();
            refilled &= filled >= out.ring.buffer().capacity() / 3;
        }
        let low = self.rate() as u64 * RING_MS as u64 / 3000;
        let dry = self.queued_end().saturating_sub(self.consumed()) < low;
        let active = self.playing && !self.timeline.ended();
        let starving = active
            && (self.starved && dry
                || underrun && !self.head_done && self.primed_at.elapsed() > PRIME_GRACE);
        if starving && !self.buffering {
            self.buffering = true;
            self.emit(EngineEvent::Buffering(true));
        } else if self.buffering && (!active || refilled) {
            self.buffering = false;
            self.emit(EngineEvent::Buffering(false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SparseCacheFile;
    use crate::decoder::tests::fixture;

    const RATE: u64 = FALLBACK_RATE as u64;
    /// One Opus frame (20 ms) at the output rate: the granularity of a decode step.
    const FRAME: u64 = RATE / 50;
    /// 2 s at 440 Hz and 1 s at 660 Hz.
    const A: &str = "sine_opus.webm";
    const B: &str = "sine2_opus.webm";

    fn src(name: &str, crossfade: bool) -> Source {
        let mut s = Source::file(name, fixture(name), 0.0);
        s.crossfade = crossfade;
        s
    }

    #[derive(Default)]
    struct Played {
        pcm: Vec<f32>,
        /// Events with the number of frames played when they were emitted.
        events: Vec<(u64, EngineEvent)>,
    }

    impl Played {
        fn frames(&self) -> u64 {
            self.pcm.len() as u64 / 2
        }

        fn at(&self, f: impl Fn(&EngineEvent) -> bool) -> u64 {
            let mut hits = self.events.iter().filter(|(_, e)| f(e));
            let (at, _) = hits.next().expect("event emitted");
            assert!(hits.next().is_none(), "emitted once");
            *at
        }

        fn ended_ms(&self, name: &str) -> u64 {
            self.events
                .iter()
                .find_map(|(_, e)| match e {
                    EngineEvent::Ended { id, played_ms, .. } if id == name => Some(*played_ms),
                    _ => None,
                })
                .expect("ended")
        }

        /// Started (`+id`) and Ended (`-id`, `-id!` when cut short) in order.
        fn story(&self) -> Vec<String> {
            let name = |id: &str| if id == A { "a" } else { "b" };
            self.events
                .iter()
                .filter_map(|(_, e)| match e {
                    EngineEvent::Started { id, .. } => Some(format!("+{}", name(id))),
                    EngineEvent::Ended { id, completed, .. } => Some(format!(
                        "-{}{}",
                        name(id),
                        if *completed { "" } else { "!" }
                    )),
                    _ => None,
                })
                .collect()
        }

        fn rms(&self, from: u64, frames: u64) -> f32 {
            let s = &self.pcm[from as usize * 2..(from + frames) as usize * 2];
            (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
        }
    }

    /// An engine state without a device, `a` as the head and `b` queued after it. Without a
    /// device `consumed` follows `written`: everything decoded counts as played at once.
    fn start(crossfade_ms: u32, a: &str, b: Option<Source>) -> (State, Receiver<EngineEvent>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut st = State::new(tx);
        st.crossfade_ms = crossfade_ms;
        let p = Pipeline::open(src(a, true), st.rate()).unwrap();
        st.start_head(p, 0, false);
        st.next_src = b;
        (st, rx)
    }

    /// Decodes until `until` frames were played or everything ended.
    fn play(st: &mut State, rx: &Receiver<EngineEvent>, out: &mut Played, until: u64) {
        let mut more = true;
        while more && st.written < until {
            st.tick();
            out.events.extend(rx.try_iter().map(|e| (st.written, e)));
            more = st.produce();
            st.written += (st.pending.len() / 2) as u64;
            out.pcm.append(&mut st.pending);
        }
        st.tick();
        out.events.extend(rx.try_iter().map(|e| (st.written, e)));
    }

    fn run(crossfade_ms: u32, a: &str, b: Source) -> Played {
        let (mut st, rx) = start(crossfade_ms, a, Some(b));
        let mut p = Played::default();
        play(&mut st, &rx, &mut p, u64::MAX);
        p
    }

    #[test]
    fn crossfade_overlaps_the_tracks() {
        let x = 300 * RATE / 1000;
        let p = run(300, A, src(B, true));
        assert!(p.frames().abs_diff(3 * RATE - x) <= FRAME, "{}", p.frames());
        assert_eq!(p.story(), ["+a", "+b", "-a", "-b"]);
        let start_b = p.at(|e| matches!(e, EngineEvent::Started { id, .. } if id == B));
        let end_a = p.at(|e| matches!(e, EngineEvent::Ended { id, .. } if id == A));
        assert!(
            start_b.abs_diff(2 * RATE - x) <= 2 * FRAME,
            "B starts at {start_b}"
        );
        assert!(end_a.abs_diff(2 * RATE) <= 2 * FRAME, "A ends at {end_a}");
        assert!((end_a - start_b).abs_diff(x) <= FRAME);
        assert!(p.ended_ms(A).abs_diff(2000) <= 20);
        assert!(p.ended_ms(B).abs_diff(1000) <= 20);

        // Equal power: mid-overlap loudness matches a single track (the cross term of two
        // sines averages out over 50 ms).
        let w = RATE / 20;
        let ratio = p.rms(2 * RATE - x / 2 - w / 2, w) / p.rms(RATE / 2, w);
        assert!((ratio - 1.0).abs() < 0.05, "power ratio {ratio}");

        // Resampled fragmented AAC, whose length is only known by counting packets (it keeps up
        // to one AAC frame of padding).
        let p = run(300, "sine_aac.m4a", src("sine2_aac.m4a", true));
        let tolerance = RATE * 24 / 1000;
        assert!(
            p.frames().abs_diff(3 * RATE - x) <= tolerance,
            "{}",
            p.frames()
        );
    }

    #[test]
    fn crossfade_is_clamped_to_half_a_track() {
        let x = RATE / 2;
        let p = run(5000, A, src(B, true));
        assert!(p.frames().abs_diff(3 * RATE - x) <= FRAME, "{}", p.frames());
        let start_b = p.at(|e| matches!(e, EngineEvent::Started { id, .. } if id == B));
        assert!(
            start_b.abs_diff(2 * RATE - x) <= 2 * FRAME,
            "B starts at {start_b}"
        );
    }

    /// A beat grid: bars of `4 · 60 / bpm` s, audible from 0 to `end_ms`.
    fn grid(bpm: f32, beat_ms: f32, end_ms: u64, outro_start_ms: u64) -> TrackAnalysis {
        TrackAnalysis {
            bpm,
            beat_ms,
            key: None,
            start_ms: 0,
            end_ms,
            intro_end_ms: end_ms,
            outro_start_ms,
        }
    }

    fn analysed(name: &str, a: TrackAnalysis, crossfade: bool) -> Source {
        let mut s = src(name, crossfade);
        s.analysis = Some(Arc::new(a));
        s
    }

    /// A (2 s) at "480 BPM" — bars of 500 ms, outro from 1.5 s — into B (1 s) at "500 BPM"
    /// with its first beat at 20 ms.
    fn smart_run(crossfade_ms: u32, b: Source) -> Played {
        let (mut st, rx) = smart_start(crossfade_ms, b);
        let mut p = Played::default();
        play(&mut st, &rx, &mut p, u64::MAX);
        p
    }

    fn smart_start(crossfade_ms: u32, b: Source) -> (State, Receiver<EngineEvent>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut st = State::new(tx);
        st.crossfade_ms = crossfade_ms;
        st.smart = true;
        let a = st
            .open_pipeline(analysed(A, grid(480.0, 0.0, 2000, 1500), true))
            .unwrap();
        st.start_head(a, 0, false);
        st.next_src = Some(b);
        (st, rx)
    }

    fn b_grid() -> TrackAnalysis {
        grid(500.0, 20.0, 1000, 900)
    }

    /// Frequency of a sine from its zero crossings.
    fn frequency(pcm: &[f32], from: u64, frames: u64) -> f64 {
        let s: Vec<f32> = (from..from + frames).map(|f| pcm[2 * f as usize]).collect();
        let crossings = s.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        crossings as f64 / 2.0 / (frames as f64 / RATE as f64)
    }

    #[test]
    fn smart_transition_starts_on_the_planned_downbeat() {
        let p = smart_run(0, analysed(B, b_grid(), true));
        // The plan: one bar (500 ms) from A's downbeat at 1.5 s; B from 20 ms at 0.96× speed
        // (120 ms beats stretched to A's 125 ms), gliding back to 1× over the last 250 ms.
        let (start, len) = (3 * RATE / 2, RATE / 2);
        let drift = (0.96 - 1.0) * (RATE as f64 / 4.0 + RATE as f64 / 8.0);
        let b_rest = (RATE as f64 * 0.98 - len as f64 - drift) as u64;
        assert_eq!(p.story(), ["+a", "+b", "-a", "-b"]);
        let start_b = p.at(|e| matches!(e, EngineEvent::Started { id, .. } if id == B));
        let end_a = p.at(|e| matches!(e, EngineEvent::Ended { id, .. } if id == A));
        assert!(start_b.abs_diff(start) <= 2 * FRAME, "B starts at {start_b}");
        assert!(end_a.abs_diff(start + len) <= 2 * FRAME, "A ends at {end_a}");
        assert!(
            p.frames().abs_diff(start + len + b_rest) <= 2 * FRAME,
            "{} frames",
            p.frames()
        );
        assert!(p.ended_ms(A).abs_diff(2000) <= 20);
        // B's positions count from where its tempo-matched start puts them.
        assert!(p.ended_ms(B).abs_diff(1000) <= 25, "{}", p.ended_ms(B));
        // After the overlap B plays at its own speed and pitch.
        let f = frequency(&p.pcm, start + len + RATE / 20, RATE / 5);
        assert!((f - 660.0).abs() < 10.0, "{f} Hz");
        // Before it, A alone.
        let f = frequency(&p.pcm, RATE / 2, RATE / 5);
        assert!((f - 440.0).abs() < 10.0, "{f} Hz");
    }

    #[test]
    fn smart_transitions_fall_back_without_analysis_and_keep_albums_gapless() {
        // Not analysed yet: the plain crossfade, exactly.
        let plain = run(300, A, src(B, true));
        let p = smart_run(300, src(B, true));
        assert!(p.pcm == plain.pcm, "plain crossfade");
        // Same album: gapless even with both analysed.
        let mut reference = Vec::new();
        for name in [A, B] {
            let mut p = Pipeline::open(src(name, true), FALLBACK_RATE).unwrap();
            while p.fill(&mut reference).unwrap() {}
        }
        let p = smart_run(300, analysed(B, b_grid(), false));
        assert!(p.pcm == reference, "gapless");
    }

    #[test]
    fn analysis_arriving_late_reaches_the_queued_successor() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut st = State::new(tx);
        st.handle(Command::Smart(true));
        let a = st
            .open_pipeline(analysed(A, grid(480.0, 0.0, 2000, 1500), true))
            .unwrap();
        st.start_head(a, 0, false);
        st.next_src = Some(src(B, true));
        st.handle(Command::Analysis {
            id: B.into(),
            analysis: Some(Arc::new(b_grid())),
        });
        let mut p = Played::default();
        play(&mut st, &rx, &mut p, u64::MAX);
        let start_b = p.at(|e| matches!(e, EngineEvent::Started { id, .. } if id == B));
        assert!(start_b.abs_diff(3 * RATE / 2) <= 2 * FRAME, "B starts at {start_b}");
    }

    #[test]
    fn no_crossfade_stays_bit_exact_gapless() {
        let mut reference = Vec::new();
        for name in [A, B] {
            let mut p = Pipeline::open(src(name, true), FALLBACK_RATE).unwrap();
            while p.fill(&mut reference).unwrap() {}
        }
        for (crossfade_ms, allowed) in [(0, true), (3000, false)] {
            let p = run(crossfade_ms, A, src(B, allowed));
            assert!(
                p.pcm == reference,
                "{crossfade_ms} ms, {allowed}: not gapless"
            );
            assert_eq!(p.story(), ["+a", "-a", "+b", "-b"]);
        }
    }

    #[test]
    fn each_side_of_a_crossfade_keeps_its_own_gain() {
        let unity = run(300, A, src(B, true));
        let (mut st, rx) = start(300, A, Some(src(B, true)));
        let mut p = Played::default();
        play(&mut st, &rx, &mut p, RATE / 2);
        // A's loudness arrives while it plays, B's while it waits as the successor.
        st.handle(Command::Gain {
            id: A.into(),
            db: -6.0,
        });
        st.handle(Command::Gain {
            id: B.into(),
            db: -12.0,
        });
        play(&mut st, &rx, &mut p, u64::MAX);
        assert_eq!(p.frames(), unity.frames());

        let w = RATE / 20;
        let ratio = |at: u64| p.rms(at, w) / unity.rms(at, w);
        assert!((ratio(RATE / 4) - 1.0).abs() < 1e-3, "before: unity");
        assert!((ratio(RATE) - 0.501).abs() < 0.01, "A: {}", ratio(RATE));
        let end = p.frames();
        let b = ratio(end - RATE / 4);
        assert!((b - 0.251).abs() < 0.01, "B: {b}");
        // Mid-overlap the mix lies between the two gains.
        let x = 300 * RATE / 1000;
        let mid = ratio(2 * RATE - x / 2 - w / 2);
        assert!(mid < 0.5 && mid > 0.25, "overlap: {mid}");
    }

    #[test]
    fn normalization_toggles_on_the_playing_track() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut st = State::new(tx);
        let quiet = Source::file(A, fixture(A), -6.0);
        let head = st.open_pipeline(quiet).unwrap();
        st.start_head(head, 0, false);
        let mut p = Played::default();
        play(&mut st, &rx, &mut p, RATE / 2);
        st.handle(Command::Normalize(false));
        play(&mut st, &rx, &mut p, RATE);
        st.handle(Command::Normalize(true));
        play(&mut st, &rx, &mut p, u64::MAX);

        let unity = run(0, A, src(B, true));
        let w = RATE / 20;
        let ratio = |at: u64| p.rms(at, w) / unity.rms(at, w);
        assert!((ratio(RATE / 4) - 0.501).abs() < 0.01, "on: {}", ratio(RATE / 4));
        assert!((ratio(3 * RATE / 4) - 1.0).abs() < 0.01, "off: {}", ratio(3 * RATE / 4));
        assert!((ratio(3 * RATE / 2) - 0.501).abs() < 0.01, "on again");
        // A new track opens with the setting as it is now.
        st.handle(Command::Normalize(false));
        let p2 = st.open_pipeline(Source::file(B, fixture(B), -6.0)).unwrap();
        st.start_head(p2, 0, false);
        let mut q = Played::default();
        play(&mut st, &rx, &mut q, RATE / 4);
        let mut reference = Vec::new();
        let mut r = Pipeline::open(src(B, true), FALLBACK_RATE).unwrap();
        while reference.len() < q.pcm.len() && r.fill(&mut reference).unwrap() {}
        assert_eq!(q.pcm[..], reference[..q.pcm.len()]);
    }

    #[test]
    fn manual_crossfade_from_the_playing_position() {
        let (mut st, rx) = start(0, A, None);
        st.set_playing(true);
        let mut p = Played::default();
        play(&mut st, &rx, &mut p, RATE / 2);
        let from = st.written;
        st.crossfade_to(src(B, true), 400);
        play(&mut st, &rx, &mut p, u64::MAX);

        let x = 400 * RATE / 1000;
        assert!(p.frames().abs_diff(from + RATE) <= FRAME, "{}", p.frames());
        assert_eq!(p.story(), ["+a", "+b", "-a!", "-b"]);
        let start_b = p.at(|e| matches!(e, EngineEvent::Started { id, .. } if id == B));
        let end_a = p.at(|e| matches!(e, EngineEvent::Ended { id, .. } if id == A));
        assert!(
            start_b - from <= FRAME,
            "B starts at {start_b}, faded from {from}"
        );
        assert!(end_a.abs_diff(from + x) <= 2 * FRAME, "A ends at {end_a}");
        assert!(p.ended_ms(A).abs_diff((from + x) * 1000 / RATE) <= 20);
    }

    #[test]
    fn seek_during_overlap_keeps_the_incoming_track() {
        let x = 400 * RATE / 1000;
        let (mut st, rx) = start(400, A, Some(src(B, true)));
        let mut p = Played::default();
        play(&mut st, &rx, &mut p, 2 * RATE - x / 2);
        assert!(
            st.fade.as_ref().is_some_and(|f| f.a.is_some()),
            "mid-overlap, A only feeds the mixer"
        );
        assert_eq!(st.slots.len(), 1);

        st.seek(200);
        p.events.extend(rx.try_iter().map(|e| (st.written, e)));
        assert!(st.fade.is_none());
        assert_eq!(st.timeline.position(st.written), Some((B, 200)));
        assert_eq!(p.story(), ["+a", "+b", "-a"]);
        assert!(
            p.events
                .iter()
                .any(|(_, e)| matches!(e, EngineEvent::Position { id, ms: 200 } if id == B))
        );

        let seeked = st.written;
        play(&mut st, &rx, &mut p, u64::MAX);
        assert_eq!(p.story(), ["+a", "+b", "-a", "-b"]);
        assert!((p.frames() - seeked).abs_diff(RATE * 8 / 10) <= FRAME);
    }

    /// Plays `st` to `RATE / 2`, seeks A to `ms` and plays the rest: frames played after the
    /// seek.
    fn seek_and_play(st: &mut State, rx: &Receiver<EngineEvent>, p: &mut Played, ms: u64) -> u64 {
        play(st, rx, p, RATE / 2);
        st.seek(ms);
        let seeked = st.written;
        play(st, rx, p, u64::MAX);
        p.frames() - seeked
    }

    #[test]
    fn seek_into_the_crossfade_tail_plays_to_the_end() {
        // The 400 ms crossfade would start at 1.6 s: sought past it, A ends, then B, gapless.
        let (mut st, rx) = start(400, A, Some(src(B, true)));
        let mut p = Played::default();
        let after = seek_and_play(&mut st, &rx, &mut p, 1800);
        assert_eq!(p.story(), ["+a", "-a", "+b", "-b"]);
        assert!(after.abs_diff(RATE * 12 / 10) <= FRAME, "{after}");
        assert!(p.ended_ms(A).abs_diff(2000) <= 20);

        // Before the tail: the crossfade as usual.
        let (mut st, rx) = start(400, A, Some(src(B, true)));
        let mut p = Played::default();
        let after = seek_and_play(&mut st, &rx, &mut p, 1000);
        assert_eq!(p.story(), ["+a", "+b", "-a", "-b"]);
        assert!(after.abs_diff(RATE * 16 / 10) <= FRAME, "{after}");
    }

    #[test]
    fn seek_into_the_smart_transition_plays_to_the_end() {
        // The smart transition would start at 1.5 s, the 300 ms crossfade at 1.7 s.
        let (mut st, rx) = smart_start(300, analysed(B, b_grid(), true));
        let mut p = Played::default();
        let after = seek_and_play(&mut st, &rx, &mut p, 1800);
        assert_eq!(p.story(), ["+a", "-a", "+b", "-b"]);
        assert!(after.abs_diff(RATE * 12 / 10) <= FRAME, "{after}");

        // Before the smart start: the planned transition on its downbeat.
        let (mut st, rx) = smart_start(300, analysed(B, b_grid(), true));
        let mut p = Played::default();
        seek_and_play(&mut st, &rx, &mut p, 1000);
        assert_eq!(p.story(), ["+a", "+b", "-a", "-b"]);
        let start_b = p.at(|e| matches!(e, EngineEvent::Started { id, .. } if id == B));
        // Sought at 0.5 s played: 1 s → 1.5 s of A is another 0.5 s.
        assert!(start_b.abs_diff(RATE) <= 2 * FRAME, "B starts at {start_b}");
    }

    /// A sparse file for fixture `name` with nothing downloaded, and the bytes to download.
    fn sparse(name: &str) -> (tempfile::TempDir, Arc<SparseCacheFile>, Arc<Vec<u8>>) {
        let data = Arc::new(std::fs::read(fixture(name)).unwrap());
        let dir = tempfile::tempdir().unwrap();
        let file = SparseCacheFile::create(dir.path().join(name), data.len() as u64).unwrap();
        (dir, file, data)
    }

    /// A downloader thread that follows the reader: `piece` bytes of the next hole every 1 ms.
    fn download(
        file: &Arc<SparseCacheFile>,
        data: &Arc<Vec<u8>>,
        piece: u64,
    ) -> std::thread::JoinHandle<()> {
        let (file, data) = (file.clone(), data.clone());
        std::thread::spawn(move || {
            while let Some(gap) = file.next_gap(file.wanted()) {
                let end = gap.end.min(gap.start + piece);
                let bytes = &data[gap.start as usize..end as usize];
                file.write_at(gap.start, bytes).unwrap();
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    }

    /// Runs the engine loop's steps until `done`. No step may wait for the download.
    fn stream(
        st: &mut State,
        rx: &Receiver<EngineEvent>,
        out: &mut Played,
        done: impl Fn(&State) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            st.tick();
            out.events.extend(rx.try_iter().map(|e| (st.written, e)));
            if done(st) {
                return;
            }
            assert!(Instant::now() < deadline, "stuck");
            st.open_waiting();
            let t = Instant::now();
            let more = st.produce();
            let took = t.elapsed();
            assert!(
                took < Duration::from_millis(100),
                "a step blocked: {took:?}"
            );
            st.written += (st.pending.len() / 2) as u64;
            out.pcm.append(&mut st.pending);
            if !more {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn buffering(p: &Played) -> Vec<bool> {
        let mut v: Vec<bool> = p
            .events
            .iter()
            .filter_map(|(_, e)| match e {
                EngineEvent::Buffering(b) => Some(*b),
                _ => None,
            })
            .collect();
        v.dedup();
        v
    }

    fn errors(p: &Played) -> Vec<&EngineEvent> {
        let err = |e: &&EngineEvent| matches!(e, EngineEvent::Error { .. });
        p.events.iter().map(|(_, e)| e).filter(err).collect()
    }

    #[test]
    fn streams_while_downloading() {
        for (name, ext) in [(A, "webm"), ("sine_aac.m4a", "m4a")] {
            let mut reference = Vec::new();
            let mut p = Pipeline::open(src(name, true), FALLBACK_RATE).unwrap();
            while p.fill(&mut reference).unwrap() {}

            let (_dir, file, data) = sparse(name);
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut st = State::new(tx);
            st.load(Source::stream(name, file.clone(), ext, 0.0), 0, true);
            assert!(
                st.waiting.is_some() && st.playing,
                "{name}: waits for the header"
            );
            let mut out = Played::default();
            stream(&mut st, &rx, &mut out, |_| true);
            assert_eq!(buffering(&out), [true], "{name}: buffering before any data");

            let dl = download(&file, &data, 512);
            stream(&mut st, &rx, &mut out, |st| st.timeline.ended());
            dl.join().unwrap();
            assert!(file.is_complete());
            assert!(errors(&out).is_empty(), "{name}: {:?}", errors(&out));
            // The download is slower than decoding: it rebuffers along the way.
            assert_eq!(buffering(&out).last(), Some(&false), "{name}");
            let first = |f: fn(&EngineEvent) -> bool| out.events.iter().position(|(_, e)| f(e));
            let started = first(|e| matches!(e, EngineEvent::Started { .. }));
            assert!(first(|e| *e == EngineEvent::Buffering(true)) < started);
            assert!(out.events.iter().any(|(_, e)| matches!(
                e,
                EngineEvent::Ended {
                    completed: true,
                    ..
                }
            )));
            let fade = (2 * RATE / 100) as usize;
            let diff: Vec<usize> = (fade..out.pcm.len().min(reference.len()))
                .filter(|&i| (out.pcm[i] - reference[i]).abs() > 1e-6)
                .collect();
            assert!(
                out.pcm.len() == reference.len() && diff.is_empty(),
                "{name}: {} frames streamed, {} from the file, {} differ from {:?}",
                out.frames(),
                reference.len() / 2,
                diff.len(),
                diff.first().map(|i| i / 2)
            );
        }
    }

    #[test]
    fn seek_into_a_missing_region_buffers() {
        let (_dir, file, data) = sparse(A);
        // The head, the first ~0.6 s and the cues at the end are there.
        file.write_at(0, &data[..7000]).unwrap();
        let tail = data.len() - 100;
        file.write_at(tail as u64, &data[tail..]).unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut st = State::new(tx);
        st.load(Source::stream(A, file.clone(), "webm", 0.0), 0, true);
        assert!(st.waiting.is_none(), "opens at once");
        let mut out = Played::default();
        stream(&mut st, &rx, &mut out, |st| st.written >= RATE / 4);
        assert!(buffering(&out).is_empty());

        st.seek(1500);
        stream(&mut st, &rx, &mut out, |st| st.starved);
        let seeked = st.written;
        stream(&mut st, &rx, &mut out, |_| true);
        assert_eq!(buffering(&out), [true]);
        assert!(
            out.events
                .iter()
                .any(|(_, e)| matches!(e, EngineEvent::Position { ms: 1500, .. }))
        );

        // Commands still work while it waits.
        st.set_playing(false);
        stream(&mut st, &rx, &mut out, |_| true);
        assert_eq!(buffering(&out), [true, false]);
        st.play();

        let dl = download(&file, &data, 1024);
        stream(&mut st, &rx, &mut out, |st| st.timeline.ended());
        dl.join().unwrap();
        assert!(errors(&out).is_empty(), "{:?}", errors(&out));
        let b = buffering(&out);
        assert!(b.starts_with(&[true, false, true]) && b.last() == Some(&false));
        let after = out.frames() - seeked;
        assert!(
            after.abs_diff(RATE / 2) <= FRAME,
            "{after} frames after the seek"
        );
        assert!(out.ended_ms(A).abs_diff(2000) <= 20);
    }

    // ---------- a fake device: the test plays the callback's part ----------

    use crate::output::Renderer;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::{Arc, Mutex};

    type Slot = Arc<Mutex<Option<(Renderer, Arc<AtomicBool>)>>>;

    struct Device {
        slot: Slot,
        opened: Arc<AtomicUsize>,
    }

    impl Device {
        /// Makes `st` open fake outputs at [`RATE`].
        fn install(st: &mut State) -> Self {
            let slot: Slot = Arc::default();
            let opened = Arc::new(AtomicUsize::new(0));
            let (s, o) = (slot.clone(), opened.clone());
            st.open_output = Box::new(move |volume, paused| {
                let (out, renderer, running) = Output::fake(RATE as u32, volume, paused);
                *s.lock().unwrap() = Some((renderer, running));
                o.fetch_add(1, Ordering::Relaxed);
                Ok(out)
            });
            Self { slot, opened }
        }

        fn opened(&self) -> usize {
            self.opened.load(Ordering::Relaxed)
        }

        fn running(&self) -> bool {
            let slot = self.slot.lock().unwrap();
            slot.as_ref().is_some_and(|(_, r)| r.load(Ordering::Acquire))
        }

        /// The device asks for `frames` in 10 ms callbacks, unless the stream is suspended.
        fn render(&self, frames: u64, pcm: &mut Vec<f32>) {
            let mut slot = self.slot.lock().unwrap();
            let Some((renderer, running)) = slot.as_mut() else {
                return;
            };
            let mut left = frames as usize;
            let mut buf = vec![0.0f32; RATE as usize / 100 * 2];
            while left > 0 && running.load(Ordering::Acquire) {
                let n = left.min(buf.len() / 2);
                renderer.render(&mut buf[..n * 2]);
                pcm.extend_from_slice(&buf[..n * 2]);
                left -= n;
            }
        }

        /// A device thread: 1 ms callbacks whenever the stream runs, until it is finished.
        fn spawn(&self) -> Callbacks {
            let stop = Arc::new(AtomicBool::new(false));
            let dev = Device {
                slot: self.slot.clone(),
                opened: Arc::default(),
            };
            let s = stop.clone();
            let thread = std::thread::spawn(move || {
                let mut pcm = Vec::new();
                while !s.load(Ordering::Acquire) {
                    dev.render(RATE / 1000, &mut pcm);
                    std::thread::sleep(Duration::from_millis(1));
                }
                pcm
            });
            Callbacks { stop, thread }
        }
    }

    struct Callbacks {
        stop: Arc<AtomicBool>,
        thread: std::thread::JoinHandle<Vec<f32>>,
    }

    impl Callbacks {
        /// Stops the thread and returns what it played.
        fn finish(self) -> Vec<f32> {
            self.stop.store(true, Ordering::Release);
            self.thread.join().unwrap()
        }
    }

    fn engine() -> (State, Receiver<EngineEvent>, Device) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut st = State::new(tx);
        let dev = Device::install(&mut st);
        (st, rx, dev)
    }

    fn load(st: &mut State, name: &str, start_ms: u64, play: bool) {
        st.handle(Command::Load {
            src: src(name, true),
            start_ms,
            play,
        });
    }

    /// Lets the "device" run for `wait` (simulated time), then runs a turn.
    fn step(st: &mut State, dev: &Device, wait: Duration, pcm: &mut Vec<f32>) {
        dev.render(RATE * wait.as_micros() as u64 / 1_000_000, pcm);
        st.turn();
    }

    /// Pretends playback stopped longer ago than the suspend delay.
    fn quiet_for_a_while(st: &mut State) {
        st.quiet_since = Some(Instant::now() - SUSPEND_AFTER);
    }

    fn position(st: &State) -> Option<(String, u64)> {
        let p = st.timeline.position(st.consumed());
        p.map(|(id, ms)| (id.to_string(), ms))
    }

    #[test]
    fn paused_load_opens_the_device_only_on_play() {
        let (mut st, _rx, dev) = engine();
        load(&mut st, A, 500, false);
        st.turn();
        assert_eq!(dev.opened(), 0);
        assert!(st.out.is_none());
        assert_eq!(st.next_wait(), None, "blocks until a command");

        st.handle(Command::Play);
        st.turn();
        assert_eq!(dev.opened(), 1);
        assert!(dev.running());
        assert_eq!(position(&st), Some((A.into(), 500)));
        assert!(st.written > 0, "primes the ring at once");
        assert!(st.next_wait().is_some_and(|w| w <= MAX_WAIT));
    }

    #[test]
    fn wakes_rarely_while_playing_and_stays_gapless() {
        let mut reference = Vec::new();
        for name in [A, B] {
            let mut p = Pipeline::open(src(name, true), FALLBACK_RATE).unwrap();
            while p.fill(&mut reference).unwrap() {}
        }
        let (mut st, rx, dev) = engine();
        load(&mut st, A, 0, true);
        st.handle(Command::SetNext(Some(src(B, true))));
        st.turn();
        let mut pcm = Vec::new();
        let (mut wakes, mut sim) = (0u32, Duration::ZERO);
        while st.playing {
            let wait = st.next_wait().expect("never blocks while playing");
            if !wait.is_zero() {
                assert!(wait >= MIN_WAIT, "{wait:?}");
                wakes += 1;
                sim += wait;
            }
            step(&mut st, &dev, wait, &mut pcm);
            if sim < Duration::from_millis(2800) {
                let shared = &st.out.as_ref().unwrap().shared;
                let underruns = shared.underruns.load(Ordering::Relaxed);
                assert_eq!(underruns, 0, "ran dry at {sim:?}");
            }
            assert!(sim < Duration::from_secs(5), "never ended");
        }
        // 3 s of audio at ≤ 10 wakeups/s, plus the last drain.
        assert!(wakes <= 33, "{wakes} wakeups");
        let events: Vec<EngineEvent> = rx.try_iter().collect();
        let story: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Started { id, .. } => Some(format!("+{id}")),
                EngineEvent::Ended { id, completed, .. } => Some(format!("-{id} {completed}")),
                _ => None,
            })
            .collect();
        let expected = [
            format!("+{A}"),
            format!("-{A} true"),
            format!("+{B}"),
            format!("-{B} true"),
        ];
        assert_eq!(story, expected);
        // Bit-exact after the start ramps, up to where the device ran past the end.
        let fade = RATE as usize / 100 * 2;
        let n = reference.len();
        assert!(pcm.len() >= n);
        let diff = (fade..n).filter(|&i| pcm[i] != reference[i]).count();
        assert_eq!(diff, 0, "{diff} samples differ");

        // Ended: suspended shortly after, closed after a while; then nothing wakes the thread.
        assert!(st.next_wait().is_some_and(|w| w <= SUSPEND_AFTER));
        quiet_for_a_while(&mut st);
        st.turn();
        assert!(!dev.running());
        let close_soon = CLOSE_AFTER - Duration::from_secs(1);
        assert!(st.next_wait().is_some_and(|w| w > close_soon));
        st.out.as_mut().unwrap().backdate_suspend(CLOSE_AFTER);
        st.turn();
        assert!(st.out.is_none());
        assert_eq!(st.next_wait(), None);

        // Play after the end starts the last track over, on a new device.
        st.handle(Command::Play);
        st.turn();
        assert_eq!(dev.opened(), 2);
        assert_eq!(position(&st), Some((B.into(), 0)));
    }

    #[test]
    fn pause_suspends_and_a_seek_while_suspended_still_flushes() {
        let (mut st, _rx, dev) = engine();
        load(&mut st, A, 0, true);
        st.turn();
        let mut pcm = Vec::new();
        for _ in 0..5 {
            let wait = st.next_wait().unwrap();
            step(&mut st, &dev, wait, &mut pcm);
        }
        st.handle(Command::Pause);
        st.turn();
        assert!(dev.running(), "the pause ramp still plays");
        assert!(st.next_wait().is_some_and(|w| !w.is_zero() && w <= SUSPEND_AFTER));
        step(&mut st, &dev, Duration::from_millis(30), &mut pcm);
        quiet_for_a_while(&mut st);
        st.turn();
        assert!(!dev.running());
        let (_, paused_at) = position(&st).unwrap();
        assert!(paused_at > 0);

        // The seek needs the callback to drop the ring: it resumes the stream for that.
        let callbacks = dev.spawn();
        st.handle(Command::Seek(1500));
        st.turn();
        let played = callbacks.finish();
        assert!(st.out.is_some(), "the flush was acknowledged");
        assert!(played.iter().all(|s| *s == 0.0), "silent while paused");
        assert_eq!(position(&st), Some((A.into(), 1500)));
        assert!(!dev.running(), "suspended again");

        st.handle(Command::Play);
        st.turn();
        assert!(dev.running());
        assert_eq!(dev.opened(), 1);
    }

    #[test]
    fn closing_an_idle_device_keeps_the_position() {
        let (mut st, rx, dev) = engine();
        load(&mut st, A, 0, true);
        st.turn();
        let mut pcm = Vec::new();
        while pcm.len() < RATE as usize {
            let wait = st.next_wait().unwrap();
            step(&mut st, &dev, wait, &mut pcm);
        }
        st.handle(Command::Pause);
        st.turn();
        step(&mut st, &dev, Duration::from_millis(30), &mut pcm);
        quiet_for_a_while(&mut st);
        st.turn();
        let at = position(&st).unwrap();
        st.out.as_mut().unwrap().backdate_suspend(CLOSE_AFTER);
        st.turn();
        assert!(st.out.is_none());
        assert_eq!(position(&st), Some(at.clone()));

        st.handle(Command::Play);
        st.turn();
        assert_eq!(dev.opened(), 2);
        assert_eq!(position(&st), Some(at));
        // Closing is not an end: the track did not report Ended.
        assert!(!rx.try_iter().any(|e| matches!(e, EngineEvent::Ended { .. })));
    }
}
