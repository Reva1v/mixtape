//! The engine thread: owns the decoders and the output, turns commands into ring contents and
//! the device's consumed-frame counter into events.
//!
//! Tracks are pushed back to back into one ring (gapless); the [`Timeline`] remembers where
//! each starts so Started/Position/Ended describe what is audible, not what is decoded.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::gain::volume_curve;
use crate::output::Output;
use crate::pipeline::Pipeline;
use crate::timeline::{Timeline, Transition};
use crate::{Result, Source};

/// Open the successor and ask for one this long before the decode head ends.
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
    /// The first sample of `id` reached the device.
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
    /// `id` is about 10 s from its end and nothing is queued after it.
    NeedNext {
        id: String,
    },
    /// `id` finished (`completed`) or was replaced/stopped.
    Ended {
        id: String,
        played_ms: u64,
        completed: bool,
    },
    /// The device ran dry while playing (slow source); `false` once refilled.
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
    SetNext(Option<Source>),
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

    /// Queues the gapless successor of the current track. `None` clears it and means "nothing
    /// follows": NeedNext is not sent again until another track becomes the decode head.
    pub fn set_next(&self, src: Option<Source>) {
        self.send(Command::SetNext(src));
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

    /// Seeks within the audible track.
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

    fn rate(&self) -> u32 {
        self.out.as_ref().map_or(FALLBACK_RATE, |o| o.rate)
    }

    fn consumed(&self) -> u64 {
        self.out
            .as_ref()
            .map_or(self.written, |o| o.shared.consumed.load(Ordering::Acquire))
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Load {
                src,
                start_ms,
                play,
            } => self.load(src, start_ms, play),
            Command::SetNext(src) => self.set_next(src),
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
        self.interrupt();
        self.flush();
        self.slots.clear();
        self.next = None;
        self.next_src = None;
        self.timeline.clear(self.rate());
        let id = src.id.clone();
        let mut p = match Pipeline::open(src, self.rate()) {
            Ok(p) => p,
            Err(e) => {
                self.emit(EngineEvent::Error {
                    id,
                    message: e.to_string(),
                });
                self.set_playing(false);
                return;
            }
        };
        if start_ms > 0
            && let Err(e) = p.seek(start_ms)
        {
            self.emit(EngineEvent::Error {
                id: id.clone(),
                message: e.to_string(),
            });
        }
        p.fade_in();
        self.start_head(p, start_ms, false);
        self.emit(EngineEvent::Position { id, ms: start_ms });
        self.primed_at = Instant::now();
        self.set_playing(play);
    }

    /// Makes `p` the decode head; its first frame lands at the current end of the ring.
    fn start_head(&mut self, mut p: Pipeline, start_ms: u64, started: bool) {
        self.seq += 1;
        p.seq = self.seq;
        let duration = p.duration_ms().unwrap_or(0);
        self.timeline
            .push_track(self.written, p.seq, &p.id, start_ms, duration, started);
        self.slots.push_back(p);
        self.head_done = false;
        self.need_next_sent = false;
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
        let playing = playing && !self.slots.is_empty();
        if let Some(o) = &self.out {
            o.shared.paused.store(!playing, Ordering::Relaxed);
        }
        if playing != self.playing {
            self.playing = playing;
            self.emit(EngineEvent::StateChanged { playing });
        }
    }

    fn seek(&mut self, ms: u64) {
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
        self.interrupt();
        self.flush();
        self.slots.clear();
        self.next = None;
        self.next_src = None;
        self.timeline.clear(self.rate());
        self.head_done = false;
        self.set_playing(false);
    }

    /// Reports the audible track as cut short.
    fn interrupt(&mut self) {
        if let Some(Transition::Ended {
            id,
            played_ms,
            completed,
        }) = self.timeline.interrupt(self.consumed())
        {
            self.emit(EngineEvent::Ended {
                id,
                played_ms,
                completed,
            });
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

    /// Flushes and restarts the audible track where the device is now.
    fn reprime_here(&mut self) {
        let consumed = self.consumed();
        let Some(ms) = self.timeline.position(consumed).map(|(_, ms)| ms) else {
            return;
        };
        let started = self.timeline.started();
        self.flush();
        self.reprime(ms, started);
    }

    /// Restarts decoding of the audible track at `ms` into an empty ring. A successor that was
    /// already decoding goes back to being the queued next.
    fn reprime(&mut self, ms: u64, started: bool) {
        self.pending.clear();
        self.pending_at = 0;
        let rate = self.rate();
        self.timeline.clear(rate);
        while self.slots.len() > 1 {
            let mut p = self.slots.pop_back().expect("len > 1");
            if self.slots.len() == 1 {
                match p.seek(0) {
                    Ok(()) => self.next = Some(p),
                    Err(e) => self.emit(EngineEvent::Error {
                        id: p.id,
                        message: e.to_string(),
                    }),
                }
            }
        }
        let Some(mut p) = self.slots.pop_front() else {
            return;
        };
        let res = p.set_out_rate(rate).and_then(|()| p.seek(ms));
        if let Err(e) = res {
            self.emit(EngineEvent::Error {
                id: p.id.clone(),
                message: e.to_string(),
            });
        }
        if let Some(mut n) = self.next.take() {
            match n.set_out_rate(rate) {
                Ok(()) => self.next = Some(n),
                Err(e) => self.emit(EngineEvent::Error {
                    id: n.id,
                    message: e.to_string(),
                }),
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
        if self.slots.is_empty() {
            return false;
        }
        if self.head_done {
            return self.advance_head();
        }
        let Some(head) = self.slots.back_mut() else {
            return false;
        };
        match head.fill(&mut self.pending) {
            Ok(more) => self.head_done = !more,
            Err(e) => {
                let (id, message) = (head.id.clone(), e.to_string());
                tracing::warn!(%id, "decode failed: {message}");
                self.emit(EngineEvent::Error { id, message });
                self.head_done = true;
            }
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
            None => {
                self.timeline.push_end(self.written);
                false
            }
        }
    }

    fn open_next(&mut self) {
        let Some(src) = self.next_src.take() else {
            return;
        };
        let id = src.id.clone();
        match Pipeline::open(src, self.rate()) {
            Ok(p) => self.next = Some(p),
            Err(e) => {
                self.emit(EngineEvent::Error {
                    id,
                    message: e.to_string(),
                });
                self.need_next_sent = false;
            }
        }
    }

    fn tick(&mut self) {
        let consumed = self.consumed();
        self.timeline.advance(consumed, &mut self.transitions);
        for t in std::mem::take(&mut self.transitions) {
            match t {
                Transition::Started {
                    seq,
                    id,
                    duration_ms,
                } => {
                    while self.slots.len() > 1 && self.slots.front().is_some_and(|p| p.seq != seq) {
                        self.slots.pop_front();
                    }
                    self.emit(EngineEvent::Started { id, duration_ms });
                }
                Transition::Ended {
                    id,
                    played_ms,
                    completed,
                } => {
                    self.emit(EngineEvent::Ended {
                        id,
                        played_ms,
                        completed,
                    });
                }
            }
        }
        if self.timeline.ended() && self.playing {
            self.set_playing(false);
        }

        let near_end = self.slots.back().is_some_and(|head| {
            self.head_done
                || head
                    .duration_ms()
                    .is_some_and(|d| d.saturating_sub(head.position_ms()) <= NEXT_LEAD_MS)
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

    fn check_buffering(&mut self) {
        let Some(out) = &self.out else { return };
        let underruns = out.shared.underruns.load(Ordering::Relaxed);
        let fresh = underruns > self.underruns_seen;
        self.underruns_seen = underruns;
        let starving = fresh
            && self.playing
            && !self.timeline.ended()
            && !self.head_done
            && self.primed_at.elapsed() > PRIME_GRACE;
        if starving && !self.buffering {
            self.buffering = true;
            self.emit(EngineEvent::Buffering(true));
        } else if self.buffering {
            let filled = out.ring.buffer().capacity() - out.ring.slots();
            if !self.playing || self.timeline.ended() || filled >= out.ring.buffer().capacity() / 3
            {
                self.buffering = false;
                self.emit(EngineEvent::Buffering(false));
            }
        }
    }
}
