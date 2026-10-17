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
//! Streamed sources ([`crate::SourceOpener::Sparse`]) never block this thread: their decoder
//! answers [`Error::Buffering`] instead of waiting for bytes, and the engine skips that decode
//! step (commands and events keep flowing) and tries again on the next turn. A load whose header
//! is not downloaded yet waits in `waiting`, a successor in `next_src`. While the head starves
//! and the ring is nearly empty, Buffering(true) is reported.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::crossfade::Mixer;
use crate::gain::volume_curve;
use crate::output::{Output, RING_MS};
use crate::pipeline::Pipeline;
use crate::timeline::{Handover, Timeline, Transition};
use crate::{Error, Result, Source};

/// Open the successor and ask for one this long (plus the crossfade) before the head ends.
const NEXT_LEAD_MS: u64 = 10_000;
const POSITION_EVERY: Duration = Duration::from_millis(100);
/// Sleep when there is nothing to decode (ring full) while playing, and while paused or idle.
const PLAYING_WAIT: Duration = Duration::from_millis(10);
const IDLE_WAIT: Duration = Duration::from_millis(250);
/// Packets decoded between command checks.
const PACKETS_PER_TURN: usize = 8;
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
    /// ~10 Hz while playing, from frames the device actually consumed.
    Position {
        id: String,
        ms: u64,
    },
    StateChanged {
        playing: bool,
    },
    /// `id` is about 10 s plus the crossfade length from its end and nothing is queued after it.
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
    Play,
    Pause,
    Toggle,
    Seek(u64),
    Volume(f32),
    Stop,
}

/// Handle to the engine thread; dropping it stops playback and joins the thread.
pub struct Engine {
    tx: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// Spawns the engine thread. The output device is opened on first use and reopened after
    /// device errors, so this succeeds without a device.
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
}

struct State {
    events: Sender<EngineEvent>,
    out: Option<Output>,
    reopen_at: Instant,
    timeline: Timeline,
    /// The audible track first, the decode head last.
    slots: VecDeque<Pipeline>,
    /// Successor opened ahead of time.
    next: Option<Pipeline>,
    next_src: Option<Source>,
    /// A crossfade into the head being decoded.
    fade: Option<Fade>,
    crossfade_ms: u32,
    /// Decoded audio waiting for room in the ring.
    pending: Vec<f32>,
    pending_at: usize,
    /// Frames pushed into the current ring.
    written: u64,
    head_done: bool,
    playing: bool,
    volume: f32,
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
}

impl State {
    fn new(events: Sender<EngineEvent>) -> Self {
        let now = Instant::now();
        Self {
            events,
            out: None,
            reopen_at: now,
            timeline: Timeline::new(FALLBACK_RATE),
            slots: VecDeque::new(),
            next: None,
            next_src: None,
            fade: None,
            crossfade_ms: 0,
            pending: Vec::new(),
            pending_at: 0,
            written: 0,
            head_done: false,
            playing: false,
            volume: 1.0,
            seq: 0,
            need_next_sent: false,
            last_position: now,
            primed_at: now,
            underruns_seen: 0,
            buffering: false,
            waiting: None,
            starved: false,
            transitions: Vec::new(),
        }
    }

    fn run(mut self, rx: Receiver<Command>) {
        let mut busy = false;
        loop {
            let wait = match (busy, self.playing) {
                (true, _) => Duration::ZERO,
                (false, true) => PLAYING_WAIT,
                (false, false) => IDLE_WAIT,
            };
            match rx.recv_timeout(wait) {
                Ok(cmd) => {
                    self.handle(cmd);
                    while let Ok(cmd) = rx.try_recv() {
                        self.handle(cmd);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.check_output();
            self.open_waiting();
            busy = false;
            for _ in 0..PACKETS_PER_TURN {
                if !self.pump() {
                    break;
                }
                busy = true;
            }
            self.tick();
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
        self.out.as_ref().map_or(FALLBACK_RATE, |o| o.rate)
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
                if ms > 0 {
                    self.slots
                        .iter_mut()
                        .chain(&mut self.next)
                        .for_each(Pipeline::measure);
                }
            }
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
            Command::Stop => self.stop(),
        }
    }

    fn load(&mut self, src: Source, start_ms: u64, play: bool) {
        self.ensure_output();
        self.reset();
        let id = src.id.clone();
        if !src.open.ready() {
            return self.wait_for(src, start_ms, play);
        }
        match Pipeline::open(src.clone(), self.rate()) {
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
        match Pipeline::open(src.clone(), self.rate()) {
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
        let b = match Pipeline::open(src.clone(), self.rate()) {
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
        self.begin_fade(b, len, Vec::new(), false);
    }

    /// Makes `p` the decode head; its first frame lands at the current end of the queue.
    fn start_head(&mut self, mut p: Pipeline, start_ms: u64, started: bool) {
        self.seq += 1;
        p.seq = self.seq;
        if self.crossfade_ms > 0 {
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
    /// start of the overlap.
    fn begin_fade(&mut self, b: Pipeline, len: u64, a_tail: Vec<f32>, completed: bool) {
        let Some(a_seq) = self.slots.back().map(|p| p.seq) else {
            return;
        };
        let mut mix = Mixer::new(len);
        mix.a.buf = a_tail;
        mix.a.ended = self.head_done;
        self.fade = Some(Fade {
            a_seq,
            a: None,
            mix,
        });
        self.start_head(b, 0, false);
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
        if let Some(o) = &self.out {
            o.shared.paused.store(!playing, Ordering::Relaxed);
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
        let Some(shared) = self.out.as_ref().map(|o| o.shared.clone()) else {
            return;
        };
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
        self.flush();
        self.reprime(ms, started);
        true
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
        // Unit tests drive the state without ever touching the real device.
        if cfg!(test) || self.out.is_some() || Instant::now() < self.reopen_at {
            return;
        }
        match Output::open(self.volume, !self.playing) {
            Ok(out) => {
                self.out = Some(out);
                self.written = 0;
                self.underruns_seen = 0;
            }
            Err(e) => {
                tracing::warn!("cannot open audio output: {e}");
                self.reopen_at = Instant::now() + REOPEN_EVERY;
            }
        }
    }

    /// Rebuilds a failed stream (device error, default device changed) and resumes the audible
    /// track where the old device stopped.
    fn check_output(&mut self) {
        let failed = self
            .out
            .as_ref()
            .is_some_and(|o| o.shared.failed.load(Ordering::Acquire));
        if failed {
            // Remember the audible position now: it is only known through the old ring.
            let resume = self.timeline.position(self.consumed()).map(|(_, ms)| ms);
            let started = self.timeline.started();
            self.out = None;
            self.written = 0;
            self.reopen_at = Instant::now();
            match resume {
                Some(ms) => self.reprime(ms, started),
                None => self.timeline.clear(self.rate()),
            }
        }
        if self.out.is_some() || self.slots.is_empty() {
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

    /// Moves one step of audio towards the ring. Returns whether anything happened.
    fn pump(&mut self) -> bool {
        let Some(out) = &mut self.out else {
            return false;
        };
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
            }
            return n > 0;
        }
        if out.ring.slots() == 0 {
            return false;
        }
        self.produce()
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
        if let Some((total, len)) = plan
            && before < total
        {
            let start = (total - len).max(before);
            let after = before + ((self.pending.len() - from) / 2) as u64;
            if after > start {
                let tail = self.pending.split_off(from + 2 * (start - before) as usize);
                let b = self.next.take().expect("planned with a successor");
                self.begin_fade(b, total - start, tail, true);
            }
        }
        true
    }

    /// Head length and overlap with the opened successor (output frames), when they crossfade.
    fn auto_fade(&self) -> Option<(u64, u64)> {
        let (a, b) = (self.slots.back()?, self.next.as_ref()?);
        if self.crossfade_ms == 0 || !b.crossfade {
            return None;
        }
        let (la, lb) = (a.out_len()?, b.out_len()?);
        let len = (self.rate() as u64 * self.crossfade_ms as u64 / 1000)
            .min(la / 2)
            .min(lb / 2);
        (len > 0).then_some((la, len))
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
        } else {
            fade.mix.mix(&mut self.pending);
            if fade.mix.finished() {
                let rest = self.fade.take().expect("fading").mix.into_rest();
                self.pending.extend_from_slice(&rest);
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
        match Pipeline::open(src.clone(), self.rate()) {
            Ok(mut p) => {
                if self.crossfade_ms > 0 && p.crossfade {
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
                || head
                    .duration_ms()
                    .is_some_and(|d| d.saturating_sub(head.position_ms()) <= lead)
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
    use std::sync::Arc;

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
}
