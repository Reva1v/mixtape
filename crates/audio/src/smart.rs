//! Smart transitions («Умные переходы»): a DJ-style blend planned from both tracks' analyses.
//!
//! The overlap starts on a downbeat in the outgoing track's outro and lasts whole bars (8 when
//! the keys go together, 2 when they clash). The incoming track starts on its first beat and is
//! played up to ±6 % faster or slower (resampling, so its pitch moves with it) so the beats
//! line up, gliding back to its own tempo over the last bar. Halfway through, the bass swaps
//! over one bar: low-shelf filters cut the incoming track's bass until then and the outgoing
//! track's after.

use std::f64::consts::PI;

use crate::analysis::TrackAnalysis;

/// The longest overlap; the engine asks for the successor this much earlier.
pub(crate) const MAX_SMART_MS: u64 = 20_000;
const MAX_STRETCH: f64 = 0.06;
/// Bars of overlap when the keys go together, and when they clash.
const BARS: u32 = 8;
const CLASH_BARS: u32 = 2;
/// Overlap when the beats cannot be matched (no steady beat, tempos too far apart).
const UNSYNCED_MS: u64 = 4_000;
/// Bass cut of the shelf filters, dB, and their corner.
const BASS_CUT_DB: f32 = -24.0;
const BASS_HZ: f32 = 180.0;
/// Filter coefficients are recomputed this often while the gain moves.
const FX_BLOCK: u64 = 32;

/// A planned transition, in output frames.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SmartPlan {
    /// Frame of the outgoing track where the overlap starts.
    pub start: u64,
    pub len: u64,
    /// Where the incoming track starts playing.
    pub b_start_ms: u64,
    /// Incoming frames consumed per output frame during the overlap (1 = untouched).
    pub speed: f64,
    /// Last frames of the overlap over which the speed glides back to 1.
    pub ramp: u64,
    /// Overlap frame where the bass swap starts, and its length.
    pub swap_at: u64,
    pub swap_len: u64,
}

impl SmartPlan {
    /// Incoming frame consumed per output frame `k` of the overlap.
    pub fn speed_at(&self, k: u64) -> f64 {
        let steady = self.len.saturating_sub(self.ramp);
        if k < steady || self.ramp == 0 {
            return self.speed;
        }
        let t = ((k - steady) as f64 / self.ramp as f64).min(1.0);
        self.speed + (1.0 - self.speed) * t
    }

    /// How much further the incoming track is after the overlap than its output length
    /// (frames, negative when it was slowed down).
    pub fn drift(&self) -> f64 {
        let steady = self.len.saturating_sub(self.ramp) as f64;
        (self.speed - 1.0) * (steady + self.ramp as f64 / 2.0)
    }

    /// Where the incoming track is when its output ends the overlap… expressed as the
    /// position its timeline should start from so positions are right once the overlap ends.
    pub fn timeline_start_ms(&self, rate: u32) -> u64 {
        let ms = self.b_start_ms as f64 + self.drift() * 1000.0 / rate as f64;
        ms.max(0.0).round() as u64
    }
}

/// Plans the transition from `a` (`a_ms` long) into `b` (`b_ms` long) at `rate`, or `None` when
/// the analyses are not usable.
pub(crate) fn plan(
    a: &TrackAnalysis,
    b: &TrackAnalysis,
    a_ms: u64,
    b_ms: u64,
    rate: u32,
) -> Option<SmartPlan> {
    if !a.usable() || !b.usable() {
        return None;
    }
    let frames = |ms: f64| (ms * rate as f64 / 1000.0).round() as u64;
    let a_end = a.end_ms.min(a_ms);

    // Tempo match, allowing half or double time.
    let synced = match (a.beat_period_ms(), b.beat_period_ms()) {
        (Some(pa), Some(pb)) => [0.5, 1.0, 2.0]
            .into_iter()
            .map(|m| (pb * m) / pa)
            .min_by(|x, y| (x - 1.0).abs().total_cmp(&(y - 1.0).abs()))
            .filter(|s| (s - 1.0).abs() <= MAX_STRETCH)
            .map(|s| (pa, s)),
        _ => None,
    };
    let compatible = match (a.key, b.key) {
        (Some(x), Some(y)) => x.compatible(y),
        _ => true,
    };

    let Some((pa, speed)) = synced else {
        // No beat matching: a short blend ending where the outro fades.
        let len_ms = UNSYNCED_MS.min(a_ms / 2).min(b_ms / 2);
        let start_ms = a
            .outro_start_ms
            .min(a_end.saturating_sub(len_ms))
            .max(a_ms / 2);
        let len = frames(len_ms as f64);
        return Some(SmartPlan {
            start: frames(start_ms as f64),
            len,
            b_start_ms: b.start_ms,
            speed: 1.0,
            ramp: 0,
            swap_at: len * 3 / 8,
            swap_len: len / 4,
        });
    };

    let bar = 4.0 * pa;
    // The incoming track's intro, in output time: a long blend over its vocals would clash.
    let b_start = first_beat_from(b, b.start_ms as f64);
    let intro = (b.intro_end_ms as f64 - b_start).max(0.0) / speed;
    let mut bars = if compatible { BARS } else { CLASH_BARS };
    let fits = |bars: u32| {
        let ms = bars as f64 * bar;
        ms <= MAX_SMART_MS as f64
            && ms <= a_ms as f64 / 2.0
            && ms * speed <= b_ms as f64 / 2.0
            && (bars <= 2 || ms <= intro.max(2.0 * bar))
    };
    while bars > 1 && !fits(bars) {
        bars /= 2;
    }
    if !fits(bars) {
        return None;
    }
    let len_ms = bars as f64 * bar;

    // The latest downbeat that starts the overlap before the outro and still ends it before
    // the outgoing track goes silent (the grid's first beat is taken as a downbeat).
    let latest = (a.outro_start_ms as f64).min(a_end as f64 - len_ms);
    let first = a.beat_ms as f64;
    if latest < first {
        return None;
    }
    let start_ms = first + ((latest - first) / bar).floor() * bar;
    if start_ms < a_ms as f64 / 2.0 {
        return None;
    }
    let len = frames(len_ms);
    let bar_frames = frames(bar);
    let swap_bar = (bars / 2).min(bars - 1) as u64;
    Some(SmartPlan {
        start: frames(start_ms),
        len,
        b_start_ms: b_start.round() as u64,
        speed,
        ramp: if speed == 1.0 {
            0
        } else {
            bar_frames.min(len / 2)
        },
        swap_at: (swap_bar * bar_frames).min(len.saturating_sub(bar_frames)),
        swap_len: bar_frames.min(len),
    })
}

/// The first beat of `t`'s grid at or after `from_ms` (or `from_ms` without a beat).
fn first_beat_from(t: &TrackAnalysis, from_ms: f64) -> f64 {
    match t.beat_period_ms() {
        Some(p) => {
            let first = t.beat_ms as f64;
            first + ((from_ms - first - 1.0) / p).ceil().max(0.0) * p
        }
        None => from_ms,
    }
}

/// How long before the end of `a` the successor must be ready for a smart transition.
pub(crate) fn lead_ms(a: &TrackAnalysis, a_ms: u64) -> u64 {
    a_ms.saturating_sub(a.outro_start_ms.min(a.end_ms)) + MAX_SMART_MS
}

/// RBJ low-shelf biquad on a stereo signal, its gain changeable while it runs.
pub(crate) struct Shelf {
    rate: f32,
    gain_db: f32,
    b: [f32; 3],
    a: [f32; 2],
    /// Transposed direct form II state per channel.
    z: [[f32; 2]; 2],
}

impl Shelf {
    pub fn new(rate: u32, gain_db: f32) -> Self {
        let mut s = Self {
            rate: rate as f32,
            gain_db: f32::NAN,
            b: [1.0, 0.0, 0.0],
            a: [0.0, 0.0],
            z: [[0.0; 2]; 2],
        };
        s.set_gain(gain_db);
        s
    }

    pub fn set_gain(&mut self, db: f32) {
        if (db - self.gain_db).abs() < 0.01 {
            return;
        }
        self.gain_db = db;
        let a = 10f32.powf(db / 40.0);
        let w = 2.0 * PI as f32 * BASS_HZ / self.rate;
        let (sin, cos) = w.sin_cos();
        // Shelf slope 1.
        let alpha = sin / 2.0 * 2f32.sqrt();
        let sa = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) + (a - 1.0) * cos + sa;
        self.b = [
            a * ((a + 1.0) - (a - 1.0) * cos + sa) / a0,
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos) / a0,
            a * ((a + 1.0) - (a - 1.0) * cos - sa) / a0,
        ];
        self.a = [
            -2.0 * ((a - 1.0) + (a + 1.0) * cos) / a0,
            ((a + 1.0) + (a - 1.0) * cos - sa) / a0,
        ];
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let mut out = [l, r];
        for (x, z) in out.iter_mut().zip(&mut self.z) {
            let y = self.b[0] * *x + z[0];
            z[0] = self.b[1] * *x - self.a[0] * y + z[1];
            z[1] = self.b[2] * *x - self.a[1] * y;
            *x = y;
        }
        (out[0], out[1])
    }
}

/// The bass swap of an overlap: per-side shelves following the plan.
pub(crate) struct BassSwap {
    at: u64,
    len: u64,
    pub a: Shelf,
    pub b: Shelf,
}

impl BassSwap {
    pub fn new(plan: &SmartPlan, rate: u32) -> Self {
        Self {
            at: plan.swap_at,
            len: plan.swap_len.max(1),
            a: Shelf::new(rate, 0.0),
            b: Shelf::new(rate, BASS_CUT_DB),
        }
    }

    /// Bass gains (dB) of the outgoing and incoming side at overlap frame `k`.
    pub fn gains(&self, k: u64) -> (f32, f32) {
        let t = (k.saturating_sub(self.at) as f32 / self.len as f32).min(1.0);
        // Equal-power-ish in dB: most of the move in the middle of the bar.
        let s = 0.5 - 0.5 * (PI as f32 * t).cos();
        (BASS_CUT_DB * s, BASS_CUT_DB * (1.0 - s))
    }

    /// Filters one frame of each side at overlap frame `k`.
    pub fn process(&mut self, k: u64, a: (f32, f32), b: (f32, f32)) -> ((f32, f32), (f32, f32)) {
        if k.is_multiple_of(FX_BLOCK) {
            let (ga, gb) = self.gains(k);
            self.a.set_gain(ga);
            self.b.set_gain(gb);
        }
        (self.a.process(a.0, a.1), self.b.process(b.0, b.1))
    }
}

/// Plays the incoming track at the plan's speed during the overlap (cubic interpolation of
/// its decoded stereo frames).
pub(crate) struct Varispeed {
    plan: SmartPlan,
    /// Decoded incoming frames, interleaved; the engine appends to it.
    pub raw: Vec<f32>,
    /// Read position in `raw`, frames.
    pos: f64,
    /// Output frames produced so far.
    done: u64,
    /// The incoming track has nothing more to decode.
    pub input_ended: bool,
}

impl Varispeed {
    pub fn new(plan: SmartPlan) -> Self {
        Self {
            plan,
            raw: Vec::new(),
            pos: 0.0,
            done: 0,
            input_ended: false,
        }
    }

    fn frames(&self) -> usize {
        self.raw.len() / 2
    }

    fn sample(&self, i: isize, ch: usize) -> f32 {
        let last = self.frames() as isize - 1;
        self.raw[i.clamp(0, last.max(0)) as usize * 2 + ch]
    }

    /// More decoded input is needed before the next output frame.
    pub fn wants_input(&self) -> bool {
        !self.input_ended && self.done < self.plan.len && (self.pos as usize) + 3 > self.frames()
    }

    /// Nothing more will come out during the overlap.
    pub fn dry(&self) -> bool {
        self.done >= self.plan.len || (self.input_ended && self.pos >= self.frames() as f64)
    }

    /// Appends as many overlap frames to `out` as the input allows.
    pub fn produce(&mut self, out: &mut Vec<f32>) {
        while self.done < self.plan.len && !self.raw.is_empty() {
            let i = self.pos as usize;
            if i + 2 >= self.frames() && !self.input_ended {
                break;
            }
            if self.pos >= self.frames() as f64 {
                break;
            }
            let f = (self.pos - i as f64) as f32;
            for ch in 0..2 {
                let i = i as isize;
                let (y0, y1, y2, y3) = (
                    self.sample(i - 1, ch),
                    self.sample(i, ch),
                    self.sample(i + 1, ch),
                    self.sample(i + 2, ch),
                );
                // Catmull-Rom.
                let c1 = 0.5 * (y2 - y0);
                let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
                let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
                out.push(((c3 * f + c2) * f + c1) * f + y1);
            }
            self.pos += self.plan.speed_at(self.done);
            self.done += 1;
        }
        // Keep one frame of history for the interpolation.
        let drop = (self.pos as usize).saturating_sub(1).min(self.frames());
        if drop > 0 {
            self.raw.drain(..drop * 2);
            self.pos -= drop as f64;
        }
    }

    /// Input left after the overlap, at the incoming track's own speed.
    pub fn into_rest(self) -> Vec<f32> {
        let from = (self.pos.round() as usize).min(self.frames());
        self.raw[from * 2..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Key;

    const RATE: u32 = 48_000;

    fn track(bpm: f32, beat_ms: f32, key: Option<Key>, ms: u64) -> TrackAnalysis {
        TrackAnalysis {
            bpm,
            beat_ms,
            key,
            start_ms: 0,
            end_ms: ms - 500,
            intro_end_ms: 30_000,
            outro_start_ms: ms.saturating_sub(20_000).max(ms / 2),
        }
    }

    fn c() -> Option<Key> {
        Some(Key::new(0, false))
    }

    #[test]
    fn plans_eight_bars_on_a_downbeat_with_matched_tempo() {
        let a = track(120.0, 100.0, c(), 200_000);
        let b = track(124.0, 250.0, Some(Key::new(7, false)), 180_000);
        let p = plan(&a, &b, 200_000, 180_000, RATE).unwrap();
        // 8 bars at 120 BPM = 16 s, on a downbeat (100 ms + k·2 s) at or before 180 s.
        assert_eq!(p.len, 16 * RATE as u64);
        assert_eq!(p.start, RATE as u64 * 178_100 / 1000);
        // The faster incoming track is slowed down to 120 BPM.
        assert!((p.speed - 120.0 / 124.0).abs() < 1e-9);
        assert_eq!(p.b_start_ms, 250);
        assert_eq!(p.ramp, 2 * RATE as u64);
        assert_eq!((p.swap_at, p.swap_len), (8 * RATE as u64, 2 * RATE as u64));
    }

    #[test]
    fn clashing_keys_blend_shorter() {
        let a = track(120.0, 0.0, c(), 200_000);
        let b = track(120.0, 0.0, Some(Key::new(1, false)), 200_000);
        let p = plan(&a, &b, 200_000, 200_000, RATE).unwrap();
        assert_eq!(p.len, 4 * RATE as u64, "2 bars");
        assert_eq!((p.speed, p.ramp), (1.0, 0));
    }

    #[test]
    fn double_time_and_too_far_tempos() {
        let a = track(87.0, 0.0, c(), 200_000);
        let b = track(174.0 * 1.03, 0.0, c(), 200_000);
        let p = plan(&a, &b, 200_000, 200_000, RATE).unwrap();
        assert!((p.speed - 1.0 / 1.03).abs() < 1e-6, "{}", p.speed);
        let b = track(100.0, 0.0, c(), 200_000);
        let p = plan(&a, &b, 200_000, 200_000, RATE).unwrap();
        assert_eq!(p.len, UNSYNCED_MS * RATE as u64 / 1000);
        assert_eq!(p.speed, 1.0);
        let mut silent = track(120.0, 0.0, c(), 200_000);
        silent.end_ms = 0;
        assert!(plan(&a, &silent, 200_000, 200_000, RATE).is_none());
    }

    #[test]
    fn a_short_intro_or_track_shortens_the_blend() {
        let a = track(120.0, 0.0, c(), 200_000);
        let mut b = track(120.0, 0.0, c(), 200_000);
        b.intro_end_ms = 5_000;
        let p = plan(&a, &b, 200_000, 200_000, RATE).unwrap();
        assert_eq!(p.len, 4 * RATE as u64, "2 bars cover what the intro allows");
        let b = track(120.0, 0.0, c(), 10_000);
        let p = plan(&a, &b, 200_000, 10_000, RATE).unwrap();
        assert!(p.len <= 5 * RATE as u64);
    }

    #[test]
    fn speed_glides_back_and_drift_adds_up() {
        let p = SmartPlan {
            start: 0,
            len: 1000,
            b_start_ms: 0,
            speed: 1.05,
            ramp: 200,
            swap_at: 0,
            swap_len: 1,
        };
        assert_eq!(p.speed_at(0), 1.05);
        assert_eq!(p.speed_at(799), 1.05);
        assert!((p.speed_at(900) - 1.025).abs() < 1e-9);
        assert_eq!(p.speed_at(1000), 1.0);
        let sum: f64 = (0..1000).map(|k| p.speed_at(k) - 1.0).sum();
        assert!((sum - p.drift()).abs() < 0.1, "{sum} vs {}", p.drift());
    }

    #[test]
    fn varispeed_reads_the_planned_amount() {
        let p = SmartPlan {
            start: 0,
            len: 48_000,
            b_start_ms: 0,
            speed: 1.04,
            ramp: 12_000,
            swap_at: 0,
            swap_len: 1,
        };
        let mut v = Varispeed::new(p.clone());
        let mut out = Vec::new();
        // A ramp 0, 1, 2, … on both channels, fed in uneven chunks.
        let total = 60_000usize;
        let mut fed = 0;
        while !v.dry() {
            if v.wants_input() {
                let n = 777.min(total - fed);
                v.raw
                    .extend((fed..fed + n).flat_map(|i| [i as f32, i as f32]));
                fed += n;
                v.input_ended = fed == total;
            }
            v.produce(&mut out);
        }
        assert_eq!(out.len(), 2 * 48_000);
        // Linear input: interpolation is exact, so output k reads input Σ speed.
        let mut pos = 0.0;
        for k in 0..48_000u64 {
            assert!((out[2 * k as usize] as f64 - pos).abs() < 0.05, "frame {k}");
            pos += p.speed_at(k);
        }
        let consumed = 48_000.0 + p.drift();
        let rest = v.into_rest();
        assert!(
            (rest[0] as f64 - consumed).abs() <= 1.0,
            "{} vs {consumed}",
            rest[0]
        );
        assert_eq!(
            rest.last().copied(),
            Some((fed - 1) as f32),
            "everything fed is kept"
        );
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn shelf_cuts_bass_and_keeps_treble() {
        let sine = |hz: f32| -> Vec<f32> {
            (0..RATE)
                .map(|i| (2.0 * PI as f32 * hz * i as f32 / RATE as f32).sin())
                .collect()
        };
        let through = |hz: f32, db: f32| {
            let mut s = Shelf::new(RATE, db);
            let out: Vec<f32> = sine(hz).into_iter().map(|x| s.process(x, x).0).collect();
            rms(&out[RATE as usize / 2..]) / rms(&sine(hz)[RATE as usize / 2..])
        };
        assert!(
            (through(50.0, 0.0) - 1.0).abs() < 1e-3,
            "0 dB is transparent"
        );
        assert!(
            through(50.0, BASS_CUT_DB) < 0.1,
            "bass cut {}",
            through(50.0, BASS_CUT_DB)
        );
        assert!(
            (through(4000.0, BASS_CUT_DB) - 1.0).abs() < 0.05,
            "treble kept"
        );
    }

    #[test]
    fn bass_swaps_over_one_bar() {
        let p = SmartPlan {
            start: 0,
            len: 8000,
            b_start_ms: 0,
            speed: 1.0,
            ramp: 0,
            swap_at: 4000,
            swap_len: 1000,
        };
        let s = BassSwap::new(&p, RATE);
        assert_eq!(s.gains(0), (0.0, BASS_CUT_DB));
        assert_eq!(s.gains(3999), (0.0, BASS_CUT_DB));
        let (a, b) = s.gains(4500);
        assert!((a - BASS_CUT_DB / 2.0).abs() < 1e-3 && (b - BASS_CUT_DB / 2.0).abs() < 1e-3);
        assert_eq!(s.gains(5000), (BASS_CUT_DB, 0.0));
        assert_eq!(s.gains(7999), (BASS_CUT_DB, 0.0));
    }

    #[test]
    fn lead_covers_the_outro() {
        let a = track(120.0, 0.0, c(), 200_000);
        assert_eq!(lead_ms(&a, 200_000), 20_000 + MAX_SMART_MS);
    }
}
