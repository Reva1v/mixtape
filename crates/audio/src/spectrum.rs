//! Spectrum for the full-screen visualiser: 16 log-spaced bands, 40 Hz … 16 kHz, in 0…1.
//!
//! The device callback writes every frame it plays as a mono sample into [`Tap`], a fixed ring
//! of atomics, before volume and the pause ramp, so the bars do not depend on the volume
//! setting. It never allocates or locks: one relaxed store per frame and one release store per
//! callback. [`SpectrumReader`] copies the latest window seqlock-style, runs a Hann-windowed
//! FFT and smooths the band levels (fast attack, slow release). When no new frames arrive
//! (paused, stopped, buffering) the bands fall back to zero.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

/// Number of bands produced by [`SpectrumReader::read`].
pub const BANDS: usize = 16;

/// Ring length in frames (power of two). Much longer than the FFT window so that the window
/// being copied is never overwritten, even with a long device buffer in flight.
const TAP_LEN: usize = 1 << 14;
const TAP_MASK: usize = TAP_LEN - 1;
/// FFT window: about 85 ms at 48 kHz, ~11.7 Hz per bin, enough for separate bass bands.
const FFT_LEN: usize = 4096;
/// Frames the writer may store ahead of the published position (one device buffer).
const IN_FLIGHT: usize = 8192;

const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;
/// Band level in dBFS mapped to 0 and to 1.
const FLOOR_DB: f32 = -60.0;
const CEIL_DB: f32 = -6.0;
/// Share of the distance to the new level covered per read (reads come at ~30 Hz).
const ATTACK: f32 = 0.6;
const RELEASE: f32 = 0.12;
/// Smoothed levels below this snap to zero.
const EPSILON: f32 = 0.002;

/// Mono samples played by the device, shared by the callback and the readers.
pub(crate) struct Tap {
    samples: Box<[AtomicU32]>,
    /// Frames written so far; samples before it are complete.
    written: AtomicU64,
    /// Device sample rate.
    rate: AtomicU32,
}

impl Tap {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            samples: (0..TAP_LEN).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            rate: AtomicU32::new(48_000),
        })
    }

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate, Ordering::Relaxed);
    }

    /// Where the next callback continues.
    pub fn position(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }

    /// Stores frame `pos`; it becomes visible with [`Tap::publish`].
    #[inline]
    pub fn store(&self, pos: u64, sample: f32) {
        self.samples[pos as usize & TAP_MASK].store(sample.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn publish(&self, pos: u64) {
        self.written.store(pos, Ordering::Release);
    }

    #[cfg(test)]
    pub fn sample(&self, pos: u64) -> f32 {
        f32::from_bits(self.samples[pos as usize & TAP_MASK].load(Ordering::Relaxed))
    }
}

/// Turns what the device plays into band levels; get one from `Engine::spectrum`.
pub struct SpectrumReader {
    tap: Arc<Tap>,
    fft: Fft,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Bin ranges of the bands, for `edges_rate`.
    edges: [usize; BANDS + 1],
    edges_rate: u32,
    levels: [f32; BANDS],
    /// Tap position at the previous read.
    last: u64,
}

impl SpectrumReader {
    pub(crate) fn new(tap: Arc<Tap>) -> Self {
        let last = tap.written.load(Ordering::Acquire);
        let window = (0..FFT_LEN)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / FFT_LEN as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        Self {
            tap,
            fft: Fft::new(FFT_LEN),
            window,
            re: vec![0.0; FFT_LEN],
            im: vec![0.0; FFT_LEN],
            edges: [0; BANDS + 1],
            edges_rate: 0,
            levels: [0.0; BANDS],
            last,
        }
    }

    /// Writes the current band levels (0…1, low to high) into `out` and returns whether any
    /// is above zero. Meant to be called at a steady ~30 Hz: the smoothing is per call.
    pub fn read(&mut self, out: &mut [f32; BANDS]) -> bool {
        let end = self.tap.written.load(Ordering::Acquire);
        let fresh = end != self.last;
        self.last = end;
        let mut target = [0.0f32; BANDS];
        if fresh {
            match self.analyse(end) {
                Some(t) => target = t,
                // The window was overwritten while copying: keep the bars as they are.
                None => {
                    *out = self.levels;
                    return self.levels.iter().any(|&v| v > 0.0);
                }
            }
        } else if self.levels.iter().all(|&v| v == 0.0) {
            *out = self.levels;
            return false;
        }
        for (level, &t) in self.levels.iter_mut().zip(&target) {
            let k = if t > *level { ATTACK } else { RELEASE };
            *level += (t - *level) * k;
            if *level < EPSILON {
                *level = 0.0;
            }
        }
        *out = self.levels;
        self.levels.iter().any(|&v| v > 0.0)
    }

    /// Unsmoothed band levels of the `FFT_LEN` frames before `end`, or `None` if the writer
    /// overtook the copy.
    fn analyse(&mut self, end: u64) -> Option<[f32; BANDS]> {
        let start = end.saturating_sub(FFT_LEN as u64);
        let missing = FFT_LEN - (end - start) as usize;
        for (i, (re, im)) in self.re.iter_mut().zip(self.im.iter_mut()).enumerate() {
            *im = 0.0;
            *re = if i < missing {
                0.0
            } else {
                let pos = start + (i - missing) as u64;
                f32::from_bits(self.tap.samples[pos as usize & TAP_MASK].load(Ordering::Relaxed))
            };
        }
        fence(Ordering::Acquire);
        let after = self.tap.written.load(Ordering::Relaxed);
        if after.wrapping_sub(end) >= (TAP_LEN - FFT_LEN - IN_FLIGHT) as u64 {
            return None;
        }
        let rate = self.tap.rate.load(Ordering::Relaxed);
        Some(self.bands(rate))
    }

    /// Windows, transforms and folds `re` into band levels.
    fn bands(&mut self, rate: u32) -> [f32; BANDS] {
        if rate != self.edges_rate {
            self.edges = band_edges(rate);
            self.edges_rate = rate;
        }
        for (s, w) in self.re.iter_mut().zip(&self.window) {
            *s *= w;
        }
        self.fft.process(&mut self.re, &mut self.im);
        // A full-scale sine has a Hann peak of N/4 and spreads over 1.5 bins of noise
        // bandwidth: this scale turns a band's summed power into its amplitude.
        let scale = 16.0 / (FFT_LEN as f32 * FFT_LEN as f32 * 1.5);
        let mut out = [0.0; BANDS];
        for (b, level) in out.iter_mut().enumerate() {
            let power: f32 = (self.edges[b]..self.edges[b + 1])
                .map(|k| self.re[k] * self.re[k] + self.im[k] * self.im[k])
                .sum();
            let amp = (power * scale).sqrt();
            let db = 20.0 * amp.max(1e-9).log10();
            *level = ((db - FLOOR_DB) / (CEIL_DB - FLOOR_DB)).clamp(0.0, 1.0);
        }
        out
    }
}

/// FFT bin ranges `[edges[b], edges[b + 1])` of the log-spaced bands at `rate`; every band
/// gets at least one bin of its own.
fn band_edges(rate: u32) -> [usize; BANDS + 1] {
    let half = FFT_LEN / 2;
    let hz_per_bin = rate.max(1) as f32 / FFT_LEN as f32;
    let ratio = (HIGH_HZ / LOW_HZ).powf(1.0 / BANDS as f32);
    let mut edges = [0; BANDS + 1];
    let mut prev = 0;
    for (i, e) in edges.iter_mut().enumerate() {
        let hz = LOW_HZ * ratio.powi(i as i32);
        let bin = ((hz / hz_per_bin).round() as usize).max(1);
        *e = if i == 0 { bin } else { bin.max(prev + 1) }.min(half);
        prev = *e;
    }
    edges
}

/// In-place iterative radix-2 complex FFT of a fixed power-of-two length.
struct Fft {
    /// `exp(-2πik/n)` for `k < n/2`.
    twiddles: Vec<(f32, f32)>,
    rev: Vec<u32>,
}

impl Fft {
    fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -std::f64::consts::TAU * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let rev = (0..n as u32)
            .map(|i| i.reverse_bits() >> (32 - bits))
            .collect();
        Self { twiddles, rev }
    }

    fn process(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.rev.len();
        debug_assert!(re.len() == n && im.len() == n);
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.twiddles[k * step];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// Feeds `frames` of a sine (or silence for `amp == 0`) into the tap, continuing at `pos`.
    fn feed(tap: &Tap, pos: &mut u64, frames: usize, hz: f32, amp: f32) {
        for _ in 0..frames {
            let t = *pos as f32 / RATE as f32;
            tap.store(*pos, amp * (std::f32::consts::TAU * hz * t).sin());
            *pos += 1;
        }
        tap.publish(*pos);
    }

    /// Band levels after `reads` reads at 30 Hz of a steady sine.
    fn settle(hz: f32, amp: f32, reads: usize) -> [f32; BANDS] {
        let tap = Tap::new();
        tap.set_rate(RATE);
        let mut reader = SpectrumReader::new(tap.clone());
        let mut pos = 0;
        let mut out = [0.0; BANDS];
        for _ in 0..reads {
            feed(&tap, &mut pos, RATE as usize / 30, hz, amp);
            reader.read(&mut out);
        }
        out
    }

    fn band_of(hz: f32) -> usize {
        let edges = band_edges(RATE);
        let bin = (hz * FFT_LEN as f32 / RATE as f32).round() as usize;
        (0..BANDS)
            .find(|&b| (edges[b]..edges[b + 1]).contains(&bin))
            .unwrap()
    }

    #[test]
    fn fft_finds_a_pure_tone() {
        let n = 64;
        let fft = Fft::new(n);
        let mut re: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 5.0 * i as f32 / n as f32).cos())
            .collect();
        let mut im = vec![0.0; n];
        fft.process(&mut re, &mut im);
        for k in 0..n {
            let mag = re[k].hypot(im[k]);
            let want = if k == 5 || k == n - 5 {
                n as f32 / 2.0
            } else {
                0.0
            };
            assert!((mag - want).abs() < 1e-3, "bin {k}: {mag}");
        }
    }

    #[test]
    fn bands_are_log_spaced_and_disjoint() {
        for rate in [44_100, 48_000, 96_000] {
            let e = band_edges(rate);
            assert!(e.windows(2).all(|w| w[0] < w[1]), "{rate}: {e:?}");
            let hz = |bin: usize| bin as f32 * rate as f32 / FFT_LEN as f32;
            assert!((hz(e[0]) - LOW_HZ).abs() < 12.0 * rate as f32 / 48_000.0);
            assert!((hz(e[BANDS]) - HIGH_HZ).abs() < 30.0 * rate as f32 / 48_000.0);
        }
    }

    #[test]
    fn a_1khz_sine_lights_its_band() {
        let out = settle(1000.0, 0.5, 20);
        let b = band_of(1000.0);
        assert_eq!(b, 8);
        assert!(out[b] > 0.85, "{out:?}");
        for (i, &v) in out.iter().enumerate() {
            if i.abs_diff(b) > 1 {
                assert!(v < 0.25, "band {i} = {v}: {out:?}");
            }
        }
        // Low and high tones land at the ends.
        let low = settle(50.0, 0.5, 20);
        assert_eq!(band_of(50.0), 0);
        assert!(low[0] > 0.8 && low[15] == 0.0, "{low:?}");
        let high = settle(12_000.0, 0.5, 20);
        assert!(high[15] > 0.8 && high[0] == 0.0, "{high:?}");
    }

    #[test]
    fn silence_reads_zero() {
        let out = settle(1000.0, 0.0, 10);
        assert_eq!(out, [0.0; BANDS]);
    }

    #[test]
    fn decays_to_zero_when_frames_stop() {
        let tap = Tap::new();
        tap.set_rate(RATE);
        let mut reader = SpectrumReader::new(tap.clone());
        let mut pos = 0;
        let mut out = [0.0; BANDS];
        for _ in 0..10 {
            feed(&tap, &mut pos, 1600, 1000.0, 0.5);
            assert!(reader.read(&mut out));
        }
        // Paused: nothing new arrives, the bars fall and then report idle.
        let mut reads = 0;
        while reader.read(&mut out) {
            reads += 1;
            assert!(reads < 80, "never settles: {out:?}");
        }
        assert!(reads > 3, "falls gently, not at once");
        assert_eq!(out, [0.0; BANDS]);
        assert!(!reader.read(&mut out));
    }

    #[test]
    fn level_follows_loudness_not_scale_errors() {
        // -40 dB quieter is clearly lower but still visible.
        let loud = settle(1000.0, 0.5, 20)[8];
        let quiet = settle(1000.0, 0.005, 20)[8];
        assert!(quiet > 0.05 && quiet < loud - 0.5, "{quiet} vs {loud}");
    }

    #[test]
    fn a_torn_copy_keeps_the_previous_bars() {
        let tap = Tap::new();
        let mut reader = SpectrumReader::new(tap.clone());
        let mut pos = 0;
        let mut out = [0.0; BANDS];
        feed(&tap, &mut pos, 4800, 1000.0, 0.5);
        reader.read(&mut out);
        let before = out;
        // The writer runs far ahead between the position load and the copy check.
        let end = tap.written.load(Ordering::Acquire);
        tap.publish(end + TAP_LEN as u64);
        assert!(reader.analyse(end).is_none());
        assert!(before.iter().any(|&v| v > 0.0));
    }
}
