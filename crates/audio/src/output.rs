//! Device output: a cpal stream (default WASAPI shared device, its default config, f32) fed from
//! an SPSC ring of interleaved stereo frames.
//!
//! The callback never allocates, locks or logs: it pops frames, copies them (pre-volume, mono)
//! into the spectrum tap, ramps pause/volume, maps stereo to the device layout and bumps an
//! atomic counter of consumed frames.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::pipeline::FADE_MS;
use crate::spectrum::Tap;
use crate::{Error, Result};

/// Ring length: enough to ride out scheduling hiccups, short enough for snappy seeks.
pub(crate) const RING_MS: u32 = 300;

/// State shared between the engine thread and the callback.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Frames popped from the ring (played or discarded by a flush).
    pub consumed: AtomicU64,
    pub paused: AtomicBool,
    /// Linear volume as `f32` bits.
    pub volume: AtomicU32,
    /// Bumped by the engine to drop everything in the ring; the callback echoes it in `flushed`.
    pub flush: AtomicU64,
    pub flushed: AtomicU64,
    /// Callbacks that found the ring empty while playing.
    pub underruns: AtomicU64,
    /// Set by the error callback: the stream must be rebuilt.
    pub failed: AtomicBool,
}

impl Shared {
    pub fn new(volume: f32) -> Arc<Self> {
        let s = Self::default();
        s.volume.store(volume.to_bits(), Ordering::Relaxed);
        Arc::new(s)
    }
}

/// The callback's side: everything it touches is preallocated.
pub(crate) struct Renderer {
    ring: Consumer<f32>,
    shared: Arc<Shared>,
    tap: Arc<Tap>,
    tap_pos: u64,
    channels: usize,
    /// Per-frame step of the pause/flush ramp.
    step: f32,
    /// Current pause gain, 0 (paused) … 1 (playing).
    level: f32,
    volume: f32,
    flush_seen: u64,
    /// Frames left in a flush fade-out.
    flush_left: u32,
    fade_frames: u32,
}

impl Renderer {
    pub fn new(
        ring: Consumer<f32>,
        shared: Arc<Shared>,
        tap: Arc<Tap>,
        channels: usize,
        rate: u32,
    ) -> Self {
        let fade_frames = (rate * FADE_MS / 1000).max(1);
        let paused = shared.paused.load(Ordering::Relaxed);
        let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
        let flush_seen = shared.flush.load(Ordering::Relaxed);
        tap.set_rate(rate);
        let tap_pos = tap.position();
        Self {
            ring,
            shared,
            tap,
            tap_pos,
            channels: channels.max(1),
            step: 1.0 / fade_frames as f32,
            level: if paused { 0.0 } else { 1.0 },
            volume,
            flush_seen,
            flush_left: 0,
            fade_frames,
        }
    }

    pub fn render(&mut self, data: &mut [f32]) {
        let flush = self.shared.flush.load(Ordering::Acquire);
        if flush != self.flush_seen && self.flush_left == 0 {
            self.flush_seen = flush;
            self.flush_left = self.fade_frames;
        }
        let paused = self.shared.paused.load(Ordering::Relaxed);
        let target_volume = f32::from_bits(self.shared.volume.load(Ordering::Relaxed));
        let volume_step =
            (target_volume - self.volume) / (data.len() / self.channels).max(1) as f32;
        let mut consumed = 0u64;
        let mut starved = false;

        for frame in data.chunks_exact_mut(self.channels) {
            if self.flush_left > 0 {
                self.flush_left -= 1;
                if self.flush_left == 0 || self.level == 0.0 {
                    consumed += self.discard();
                    self.flush_left = 0;
                    self.shared
                        .flushed
                        .store(self.flush_seen, Ordering::Release);
                    frame.fill(0.0);
                    continue;
                }
            }
            let fading_out = paused || self.flush_left > 0;
            if fading_out && self.level == 0.0 {
                frame.fill(0.0);
                continue;
            }
            let (l, r) = match (self.ring.pop(), self.ring.pop()) {
                (Ok(l), Ok(r)) => (l, r),
                _ => {
                    starved = true;
                    frame.fill(0.0);
                    continue;
                }
            };
            consumed += 1;
            // Before volume and ramps: the spectrum must not follow the volume knob.
            self.tap.store(self.tap_pos, 0.5 * (l + r));
            self.tap_pos += 1;
            let ramp = if self.flush_left > 0 {
                self.flush_left as f32 / self.fade_frames as f32
            } else {
                1.0
            };
            self.level = if fading_out {
                // Snap so rounding never leaves the ramp hovering just above zero.
                let l = self.level - self.step;
                if l < self.step * 0.5 { 0.0 } else { l }
            } else {
                (self.level + self.step).min(1.0)
            };
            self.volume += volume_step;
            let g = self.level * self.volume * ramp;
            write_frame(frame, l * g, r * g);
        }
        self.volume = target_volume;
        if consumed > 0 {
            self.shared.consumed.fetch_add(consumed, Ordering::Release);
            self.tap.publish(self.tap_pos);
        }
        if starved && !paused {
            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn discard(&mut self) -> u64 {
        let n = self.ring.slots() & !1;
        if let Ok(chunk) = self.ring.read_chunk(n) {
            chunk.commit_all();
        }
        (n / 2) as u64
    }
}

#[inline]
fn write_frame(frame: &mut [f32], l: f32, r: f32) {
    match frame {
        [m] => *m = 0.5 * (l + r),
        [fl, fr, rest @ ..] => {
            *fl = l;
            *fr = r;
            rest.fill(0.0);
        }
        [] => {}
    }
}

/// A running device stream plus the producer side of its ring.
pub(crate) struct Output {
    _stream: cpal::Stream,
    pub ring: Producer<f32>,
    pub shared: Arc<Shared>,
    pub rate: u32,
}

impl Output {
    /// Opens the default output device. `shared` carries pause/volume across rebuilds; its
    /// counters are reset because the ring is new.
    pub fn open(volume: f32, paused: bool, tap: Arc<Tap>) -> Result<Self> {
        let err = |e: &dyn std::fmt::Display| Error::Output(e.to_string());
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| Error::Output("no output device".into()))?;
        let config = device
            .default_output_config()
            .map_err(|e| err(&e))?
            .config();
        let rate = config.sample_rate;
        let channels = config.channels as usize;
        let shared = Shared::new(volume);
        shared.paused.store(paused, Ordering::Relaxed);

        let (ring, consumer) = RingBuffer::new((rate * RING_MS / 1000) as usize * 2);
        let mut renderer = Renderer::new(consumer, shared.clone(), tap, channels, rate);
        let on_error = {
            let shared = shared.clone();
            move |e: cpal::Error| {
                tracing::warn!("audio stream error: {e}");
                if !matches!(
                    e.kind(),
                    cpal::ErrorKind::Xrun
                        | cpal::ErrorKind::RealtimeDenied
                        | cpal::ErrorKind::DeviceChanged
                ) {
                    shared.failed.store(true, Ordering::Release);
                }
            }
        };
        let stream = device
            .build_output_stream::<f32, _, _>(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| renderer.render(data),
                on_error,
                None,
            )
            .map_err(|e| err(&e))?;
        stream.play().map_err(|e| err(&e))?;
        tracing::info!(rate, channels, "audio output opened");
        Ok(Self {
            _stream: stream,
            ring,
            shared,
            rate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(frames: usize) -> (Producer<f32>, Renderer, Arc<Shared>) {
        setup_with(frames, Tap::new())
    }

    fn setup_with(frames: usize, tap: Arc<Tap>) -> (Producer<f32>, Renderer, Arc<Shared>) {
        let (mut tx, rx) = RingBuffer::new(4096);
        for i in 0..frames {
            tx.push(i as f32).unwrap();
            tx.push(-(i as f32)).unwrap();
        }
        let shared = Shared::new(1.0);
        let r = Renderer::new(rx, shared.clone(), tap, 2, 1000);
        (tx, r, shared)
    }

    #[test]
    fn plays_counts_and_maps_channels() {
        let (_tx, mut r, shared) = setup(100);
        let mut out = [9.0f32; 20];
        r.render(&mut out);
        assert_eq!(&out[..4], &[0.0, 0.0, 1.0, -1.0]);
        assert_eq!(shared.consumed.load(Ordering::Relaxed), 10);

        let mut surround = [9.0f32; 12];
        r.channels = 6;
        r.render(&mut surround);
        assert_eq!(&surround[..6], &[10.0, -10.0, 0.0, 0.0, 0.0, 0.0]);
        r.channels = 1;
        let mut mono = [9.0f32; 1];
        r.render(&mut mono);
        assert_eq!(mono, [0.0]);
    }

    #[test]
    fn underrun_writes_silence_without_consuming() {
        let (_tx, mut r, shared) = setup(2);
        let mut out = [9.0f32; 8];
        r.render(&mut out);
        assert_eq!(out[4..], [0.0; 4]);
        assert_eq!(shared.consumed.load(Ordering::Relaxed), 2);
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn pause_fades_out_and_keeps_the_rest() {
        // 1 kHz device: the fade is 10 frames.
        let (_tx, mut r, shared) = setup(100);
        shared.paused.store(true, Ordering::Relaxed);
        let mut out = [9.0f32; 60];
        r.render(&mut out);
        assert_eq!(shared.consumed.load(Ordering::Relaxed), 10);
        assert_eq!(out[20..], [0.0; 40]);
        assert!(out[2].abs() > out[18].abs() || out[18] == 0.0);

        shared.paused.store(false, Ordering::Relaxed);
        r.render(&mut out);
        assert_eq!(shared.consumed.load(Ordering::Relaxed), 40);
        assert!(out[0].abs() < 10.0 * 0.11, "fades back in: {}", out[0]);
    }

    #[test]
    fn flush_fades_then_discards() {
        let (_tx, mut r, shared) = setup(100);
        shared.flush.store(1, Ordering::Release);
        let mut out = [9.0f32; 40];
        r.render(&mut out);
        assert_eq!(shared.flushed.load(Ordering::Acquire), 1);
        assert_eq!(shared.consumed.load(Ordering::Relaxed), 100);
        assert_eq!(out[20..], [0.0; 20]);
    }

    #[test]
    fn spectrum_tap_ignores_volume_and_pause_ramp() {
        let tap_of = |volume: f32| {
            let (mut tx, rx) = RingBuffer::new(4096);
            for i in 0..100 {
                tx.push(i as f32).unwrap();
                tx.push(i as f32 + 0.5).unwrap();
            }
            let shared = Shared::new(volume);
            let tap = Tap::new();
            let mut r = Renderer::new(rx, shared.clone(), tap.clone(), 2, 1000);
            let mut out = [9.0f32; 40];
            r.render(&mut out);
            shared.paused.store(true, Ordering::Relaxed);
            r.render(&mut out);
            r.render(&mut out);
            (
                tap.position(),
                (0..30).map(|i| tap.sample(i)).collect::<Vec<_>>(),
            )
        };
        let (n_loud, loud) = tap_of(1.0);
        let (n_quiet, quiet) = tap_of(0.1);
        // 20 frames played, 10 more while the pause ramp fades out, then nothing.
        assert_eq!(n_loud, 30);
        assert_eq!((n_loud, &loud), (n_quiet, &quiet));
        // Mono mix of the raw frames, untouched by the ramp.
        assert_eq!(loud[3], 3.25);
        assert_eq!(loud[29], 29.25);
    }
}
