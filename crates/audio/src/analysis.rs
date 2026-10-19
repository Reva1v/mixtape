//! One-time analysis of a track for smart transitions: tempo and beat grid (spectral flux,
//! autocorrelation for the tempo, then the constant-tempo grid whose folded onsets peak
//! sharpest; octave errors settled by where the snares fall; downbeats from kicks and
//! spectral change per beat, phrases from change per bar), key (a per-note chroma against
//! the Krumhansl–Kessler profiles), and where the audible part, the intro and the outro are
//! (a loudness envelope in 250 ms blocks).
//!
//! Everything runs on a mono copy decimated to ~11 kHz, so a 4-minute track takes well under a
//! second of one core; callers run it on a background thread.

use std::path::Path;

use crate::decoder::Decoder;
use crate::fft::Fft;
use crate::{Result, SourceOpener};

/// Bumped when the analysis changes enough that stored results should be redone.
pub const ANALYSIS_VERSION: u32 = 2;

const TARGET_RATE: f64 = 11_025.0;
/// Onset analysis: window and hop in samples at the analysis rate (~93 ms / ~23 ms).
const WIN: usize = 1024;
const HOP: usize = 256;
/// Chroma analysis window (~370 ms, ~2.7 Hz bins), hopped by half.
const CHROMA_WIN: usize = 4096;
const BLOCK_MS: u64 = 250;
/// Blocks quieter than this (RMS, dBFS) are silence.
const SILENCE_DB: f32 = -50.0;
/// The intro ends and the outro starts where the smoothed level is within this of the
/// track's typical loud level.
const BODY_DB: f32 = 6.0;
/// Smoothing of the envelope, blocks (2 s).
const SMOOTH_BLOCKS: usize = 8;
const MIN_BPM: f64 = 60.0;
const MAX_BPM: f64 = 200.0;
/// Tempos are read within this range (BPM): half or double of it is the same grid.
const PREFERRED: (f64, f64) = (78.0, 180.0);
/// Slower readings up to this may be double time ([`DOUBLE_RATIO`] decides).
const DOUBLE_BELOW: f64 = 90.0;
/// Snare/hi-hat onsets halfway between the slow beats against those on them from which the
/// tempo is double.
pub const DOUBLE_RATIO: f64 = 0.95;
/// Onset analysis bands (Hz): kick and bass below `LOW_HZ`, snares and hi-hats above
/// `HIGH_HZ`.
const LOW_HZ: f64 = 150.0;
const HIGH_HZ: f64 = 1500.0;
/// Coarse spectrum for downbeats and phrases: band edges (Hz).
const BANDS: usize = 7;
const BAND_EDGES: [f64; BANDS + 1] = [40.0, 80.0, 160.0, 320.0, 640.0, 1280.0, 2560.0, 5000.0];
/// The bands that carry bass notes and chords (below 640 Hz).
const HARMONIC_BANDS: usize = 4;
/// Bins of one beat period when onsets are folded onto it.
const BINS: usize = 32;
/// A steady beat: folded onsets peak this much above their average…
pub const MIN_CONTRAST: f64 = 1.6;
/// …and the beat phase of 4-bar stretches agrees at least this much…
pub const MIN_CONSISTENCY: f64 = 0.5;
/// …and the tempo holds in each stretch (rubato and drift fall well below)…
pub const MIN_STABILITY: f64 = 0.9;
/// …and the onsets on the beat are this strong (a steady tone has regular but tiny ones).
pub const MIN_BEAT_ONSET: f64 = 3.0;
/// …and the onsets repeat beat by beat (a few loud events alone can line up with a grid).
pub const MIN_PERIODICITY: f64 = 0.1;
/// …and most beats have an onset on them.
pub const MIN_COVERAGE: f64 = 0.3;
/// Bars of a phrase, and how much the change at the phrase start must stand out from the
/// other bars to trust it (else phrases count from the first downbeat of the music).
const PHRASE_BARS: usize = 8;
const MIN_PHRASE: f64 = 1.3;

/// A musical key: tonic pitch class (0 = C … 11 = B) and mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key(u8);

impl Key {
    pub fn new(tonic: u8, minor: bool) -> Self {
        Self(tonic % 12 + if minor { 12 } else { 0 })
    }

    /// `0..12` major keys by tonic, `12..24` minor keys.
    pub fn index(self) -> u8 {
        self.0
    }

    pub fn from_index(i: u8) -> Option<Self> {
        (i < 24).then_some(Self(i))
    }

    pub fn tonic(self) -> u8 {
        self.0 % 12
    }

    pub fn minor(self) -> bool {
        self.0 >= 12
    }

    /// Position on the Camelot wheel (1…12; relative major and minor share it).
    pub fn camelot(self) -> u8 {
        let major_tonic = if self.minor() {
            (self.tonic() + 3) % 12
        } else {
            self.tonic()
        };
        match (7 * major_tonic as u32 + 8) % 12 {
            0 => 12,
            n => n as u8,
        }
    }

    /// Harmonic mixing: the same key, its relative major/minor, or a fifth up or down.
    pub fn compatible(self, other: Key) -> bool {
        let (a, b) = (self.camelot(), other.camelot());
        if a == b {
            return true;
        }
        let step = (a as i32 - b as i32).rem_euclid(12);
        self.minor() == other.minor() && (step == 1 || step == 11)
    }
}

/// What smart transitions need to know about a track. Times are from the start of the track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackAnalysis {
    /// Tempo; 0 when there is no steady beat.
    pub bpm: f32,
    /// A downbeat of the (constant-tempo) grid that starts a phrase (8 bars), within the first
    /// phrase: beats are every period from it, bars every 4 beats, phrases every 32 (also
    /// before it).
    pub beat_ms: f32,
    pub key: Option<Key>,
    /// First and last audible moment (leading and trailing silence excluded); `end_ms` 0 means
    /// the track is silent or could not be analysed.
    pub start_ms: u64,
    pub end_ms: u64,
    /// Where the track reaches its usual loudness, and where it finally drops below it.
    pub intro_end_ms: u64,
    pub outro_start_ms: u64,
}

impl TrackAnalysis {
    /// Nothing usable: smart transitions fall back to the plain crossfade.
    pub fn empty() -> Self {
        Self {
            bpm: 0.0,
            beat_ms: 0.0,
            key: None,
            start_ms: 0,
            end_ms: 0,
            intro_end_ms: 0,
            outro_start_ms: 0,
        }
    }

    pub fn usable(&self) -> bool {
        self.end_ms > self.start_ms
    }

    pub fn beat_period_ms(&self) -> Option<f64> {
        (self.bpm > 0.0).then(|| 60_000.0 / self.bpm as f64)
    }
}

/// Decodes the whole file and analyses it.
pub fn analyze_file(path: &Path) -> Result<TrackAnalysis> {
    let (x, rate) = decode_decimated(path)?;
    Ok(analyze_decimated(&x, rate))
}

/// The whole file as mono at the analysis rate.
fn decode_decimated(path: &Path) -> Result<(Vec<f32>, f64)> {
    let hint = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let mut dec = Decoder::open(SourceOpener::File(path.to_path_buf()), hint.as_deref())?;
    let mut down = Decimator::new(dec.rate());
    let mut pcm = Vec::new();
    loop {
        pcm.clear();
        let more = dec.next_chunk(&mut pcm)?;
        for f in pcm.chunks_exact(2) {
            down.push((f[0] + f[1]) * 0.5);
        }
        if !more {
            break;
        }
    }
    Ok((down.out, down.rate))
}

/// Analyses mono samples at `rate`.
pub fn analyze_pcm(mono: &[f32], rate: u32) -> TrackAnalysis {
    let mut down = Decimator::new(rate);
    for &x in mono {
        down.push(x);
    }
    let rate = down.rate;
    analyze_decimated(&down.out, rate)
}

/// Box-filter decimation by an integer factor towards [`TARGET_RATE`]. Crude, but the
/// analysis only looks below ~5 kHz.
struct Decimator {
    factor: usize,
    rate: f64,
    acc: f32,
    n: usize,
    out: Vec<f32>,
}

impl Decimator {
    fn new(rate: u32) -> Self {
        let factor = ((rate as f64 / TARGET_RATE).round() as usize).max(1);
        Self {
            factor,
            rate: rate as f64 / factor as f64,
            acc: 0.0,
            n: 0,
            out: Vec::new(),
        }
    }

    fn push(&mut self, x: f32) {
        self.acc += x;
        self.n += 1;
        if self.n == self.factor {
            self.out.push(self.acc / self.factor as f32);
            self.acc = 0.0;
            self.n = 0;
        }
    }
}

fn analyze_decimated(x: &[f32], rate: f64) -> TrackAnalysis {
    let mut a = TrackAnalysis::empty();
    if !envelope(x, rate, &mut a) {
        return a;
    }
    if let Some((bpm, beat_ms)) = tempo(x, rate, a.start_ms) {
        a.bpm = bpm as f32;
        a.beat_ms = beat_ms as f32;
    }
    a.key = key(x, rate);
    a
}

/// Fills the start/end and intro/outro marks; `false` for a silent track.
fn envelope(x: &[f32], rate: f64, a: &mut TrackAnalysis) -> bool {
    let block = ((rate * BLOCK_MS as f64 / 1000.0) as usize).max(1);
    let floor = SILENCE_DB - 10.0;
    let db: Vec<f32> = x
        .chunks(block)
        .map(|c| {
            let ms = c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32;
            (10.0 * (ms + 1e-12).log10()).max(floor)
        })
        .collect();
    let (Some(first), Some(last)) = (
        db.iter().position(|&d| d > SILENCE_DB),
        db.iter().rposition(|&d| d > SILENCE_DB),
    ) else {
        return false;
    };
    let duration_ms = (x.len() as f64 * 1000.0 / rate) as u64;
    a.start_ms = first as u64 * BLOCK_MS;
    a.end_ms = ((last as u64 + 1) * BLOCK_MS).min(duration_ms);

    let mut levels: Vec<f32> = db[first..=last]
        .iter()
        .copied()
        .filter(|&d| d > SILENCE_DB)
        .collect();
    levels.sort_by(f32::total_cmp);
    let typical = levels[(levels.len() - 1) * 4 / 5];
    let body = typical - BODY_DB;
    let half = SMOOTH_BLOCKS / 2;
    let smooth = |i: usize| {
        let (lo, hi) = (i.saturating_sub(half), (i + half).min(db.len() - 1));
        db[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32
    };
    let intro = (first..=last).find(|&i| smooth(i) >= body).unwrap_or(first);
    let outro = (first..=last)
        .rev()
        .find(|&i| smooth(i) >= body)
        .unwrap_or(last);
    a.intro_end_ms = (intro as u64 * BLOCK_MS).max(a.start_ms);
    a.outro_start_ms = ((outro as u64 + 1) * BLOCK_MS).clamp(a.intro_end_ms, a.end_ms);
    true
}

/// Onset strength (spectral flux of log magnitudes, one value per [`HOP`]) of the whole band,
/// the kick/bass band and the snare/hi-hat band, and a coarse log spectrum per frame.
struct Onsets {
    full: Vec<f32>,
    low: Vec<f32>,
    high: Vec<f32>,
    bands: Vec<[f32; BANDS]>,
}

fn onsets(x: &[f32], rate: f64) -> Onsets {
    let mut o = Onsets {
        full: Vec::new(),
        low: Vec::new(),
        high: Vec::new(),
        bands: Vec::new(),
    };
    if x.len() < WIN {
        return o;
    }
    let mut fft = Fft::new(WIN);
    let bin = |hz: f64| ((hz / rate * WIN as f64) as usize).clamp(1, WIN / 2);
    let top = bin(5000.0);
    let (low, high) = (bin(LOW_HZ).max(2), bin(HIGH_HZ));
    let edges = BAND_EDGES.map(bin);
    let mut mag = Vec::new();
    let mut prev = vec![0f32; top];
    let mut m = vec![0f32; top];
    let frames = (x.len() - WIN) / HOP + 1;
    for v in [&mut o.full, &mut o.low, &mut o.high] {
        v.reserve(frames);
    }
    o.bands.reserve(frames);
    for start in (0..=x.len() - WIN).step_by(HOP) {
        fft.magnitudes(&x[start..start + WIN], &mut mag);
        let (mut full, mut lo, mut hi) = (0.0, 0.0, 0.0);
        for k in 1..top {
            m[k] = (1.0 + 100.0 * mag[k]).ln();
            let d = (m[k] - prev[k]).max(0.0);
            full += d;
            if k < low {
                lo += d;
            } else if k >= high {
                hi += d;
            }
        }
        prev.copy_from_slice(&m);
        o.full.push(full);
        o.low.push(lo);
        o.high.push(hi);
        let mut b = [0f32; BANDS];
        for (i, v) in b.iter_mut().enumerate() {
            let (lo, hi) = (edges[i], edges[i + 1].max(edges[i] + 1).min(top));
            *v = m[lo..hi].iter().sum::<f32>() / (hi - lo).max(1) as f32;
        }
        o.bands.push(b);
    }
    for v in [&mut o.full, &mut o.low, &mut o.high] {
        *v = above_local_mean(v, (rate / HOP as f64) as usize);
    }
    o
}

/// `v` above its moving mean over `w` values, half-wave rectified.
fn above_local_mean(v: &[f32], w: usize) -> Vec<f32> {
    let n = v.len();
    let mut sums = vec![0f64; n + 1];
    for i in 0..n {
        sums[i + 1] = sums[i] + v[i] as f64;
    }
    (0..n)
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(w / 2), (i + w / 2 + 1).min(n));
            let mean = (sums[hi] - sums[lo]) / (hi - lo) as f64;
            (v[i] as f64 - mean).max(0.0) as f32
        })
        .collect()
}

/// Linear interpolation of `v` at a fractional index (0 outside).
fn at(v: &[f32], x: f64) -> f64 {
    if x < 0.0 {
        return 0.0;
    }
    let i = x as usize;
    if i + 1 >= v.len() {
        return v.get(i).copied().unwrap_or(0.0) as f64;
    }
    let f = x - i as f64;
    v[i] as f64 * (1.0 - f) + v[i + 1] as f64 * f
}

/// Tempo (BPM) and a phrase-starting downbeat (ms), if the beat is steady.
fn tempo(x: &[f32], rate: f64, start_ms: u64) -> Option<(f64, f64)> {
    let t = tempo_detail(x, rate, start_ms)?;
    t.steady().then_some((t.bpm, t.beat_ms))
}

/// The best beat grid and how steady it is, before the steadiness thresholds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoDetail {
    pub bpm: f64,
    /// A downbeat that starts a phrase (see [`TrackAnalysis::beat_ms`]).
    pub beat_ms: f64,
    /// Autocorrelation of the onsets one or two beats apart relative to lag 0: there is a
    /// pulse at all (≥ [`MIN_PERIODICITY`]).
    pub periodicity: f64,
    /// Onsets folded onto one beat period of the fitted grid: the peak against the average.
    /// Flat (≈ 1) without a steady beat; a steady beat needs ≥ [`MIN_CONTRAST`].
    pub strength: f64,
    /// How well the beat phase of 4-bar stretches agrees along the track (0…1, weighted by
    /// how clear each stretch is); a steady beat needs ≥ [`MIN_CONSISTENCY`].
    pub consistency: f64,
    /// How well the tempo holds in each 4-bar stretch: its fold peak against the sharpest of
    /// nearby tempos (≈ 1 steady, lower for rubato; ≥ [`MIN_STABILITY`]).
    pub stability: f64,
    /// Snare/hi-hat onsets halfway between the beats of the slower reading against those on
    /// them, when the tempo could also be read double (≥ [`DOUBLE_RATIO`]: double).
    pub half_beat: Option<f64>,
    /// Onsets on the beat above the average, per beat (spectral flux units): the beat must
    /// be heard, not just regular (≥ [`MIN_BEAT_ONSET`]).
    pub beat_onset: f64,
    /// Share of the grid's beats with an onset on them above the track's average onset
    /// (≥ [`MIN_COVERAGE`]): a handful of loud events can line up with a grid too.
    pub coverage: f64,
    /// Downbeat class against the average of the four, and the phrase offset likewise.
    pub downbeat: f64,
    pub phrase: f64,
}

impl TempoDetail {
    pub fn steady(&self) -> bool {
        self.strength >= MIN_CONTRAST
            && self.consistency >= MIN_CONSISTENCY
            && self.stability >= MIN_STABILITY
            && self.beat_onset >= MIN_BEAT_ONSET
            && self.periodicity >= MIN_PERIODICITY
            && self.coverage >= MIN_COVERAGE
    }
}

/// A constant-tempo grid in onset frames.
#[derive(Debug, Clone, Copy)]
struct Grid {
    period: f64,
    /// A beat, within the first period.
    phase: f64,
    contrast: f64,
    /// Folded onsets at the peak above their average, per beat.
    excess: f64,
}

/// Onsets `o[range]` folded onto one `period` ([`BINS`] bins, bin 0 at phase 0), lightly
/// smoothed.
fn fold(o: &[f32], range: std::ops::Range<usize>, period: f64) -> [f64; BINS] {
    let mut h = [0f64; BINS];
    let scale = BINS as f64 / period;
    for i in range {
        let v = o[i];
        if v <= 0.0 {
            continue;
        }
        let p = (i as f64 * scale) % BINS as f64;
        let b = p as usize;
        let f = p - b as f64;
        h[b % BINS] += v as f64 * (1.0 - f);
        h[(b + 1) % BINS] += v as f64 * f;
    }
    std::array::from_fn(|b| 0.5 * h[b] + 0.25 * (h[(b + BINS - 1) % BINS] + h[(b + 1) % BINS]))
}

/// The peak of a folded histogram: (fractional bin, height).
fn fold_peak(h: &[f64; BINS]) -> (f64, f64) {
    let b = (0..BINS)
        .max_by(|&x, &y| h[x].total_cmp(&h[y]))
        .unwrap_or(0);
    let (y0, y1, y2) = (h[(b + BINS - 1) % BINS], h[b], h[(b + 1) % BINS]);
    let d = y0 - 2.0 * y1 + y2;
    let off = if d.abs() > 1e-12 {
        (0.5 * (y0 - y2) / d).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    ((b as f64 + off).rem_euclid(BINS as f64), y1)
}

/// Value of a folded histogram at a fractional bin (circular, linear).
fn fold_at(h: &[f64; BINS], bin: f64) -> f64 {
    let p = bin.rem_euclid(BINS as f64);
    let b = p as usize % BINS;
    let f = p - p.floor();
    h[b] * (1.0 - f) + h[(b + 1) % BINS] * f
}

/// The constant-tempo grid that lines up most onsets, searched within ±3 % of `period`: the
/// period whose folded onsets have the sharpest peak, and the phase of that peak.
fn fit_grid(o: &[f32], period: f64) -> Grid {
    let n = o.len();
    let sharpest = |from: f64, to: f64, step: f64| {
        let mut best = (period, -1.0);
        let mut r = from;
        while r <= to + 1e-12 {
            let p = period * (1.0 + r);
            let peak = fold_peak(&fold(o, 0..n, p)).1;
            if peak > best.1 {
                best = (p, peak);
            }
            r += step;
        }
        best.0
    };
    let coarse = sharpest(-0.03, 0.03, 0.0002);
    let r = coarse / period - 1.0;
    let period = sharpest(r - 0.0002, r + 0.0002, 0.00002);
    let h = fold(o, 0..n, period);
    let (bin, peak) = fold_peak(&h);
    let mean = h.iter().sum::<f64>() / BINS as f64;
    Grid {
        period,
        phase: bin * period / BINS as f64,
        contrast: if mean > 0.0 { peak / mean } else { 0.0 },
        excess: (peak - mean) * period / n as f64,
    }
}

fn tempo_detail(x: &[f32], rate: f64, start_ms: u64) -> Option<TempoDetail> {
    let on = onsets(x, rate);
    let o = &on.full;
    let fr = rate / HOP as f64;
    let n = o.len();
    if (n as f64) < fr * 8.0 || o.iter().all(|&v| v <= 0.0) {
        return None;
    }

    // Autocorrelation, then the tempo whose first four multiples line up best, preferring
    // tempos near 120 BPM against octave errors.
    let max_lag = ((4.0 * 60.0 * fr / MIN_BPM).ceil() as usize + 2).min(n - 1);
    let acf: Vec<f32> = (0..=max_lag)
        .map(|l| {
            let s: f64 = (0..n - l).map(|i| o[i] as f64 * o[i + l] as f64).sum();
            (s / (n - l) as f64) as f32
        })
        .collect();
    let mut best = (0.0, 0.0);
    let mut bpm = MIN_BPM;
    while bpm <= MAX_BPM {
        let lag = 60.0 * fr / bpm;
        let comb: f64 = (1..=4).map(|k| at(&acf, k as f64 * lag)).sum();
        let weight = (-0.5 * (bpm / 120.0).log2().powi(2)).exp();
        if comb * weight > best.1 {
            best = (bpm, comb * weight);
        }
        bpm += 0.1;
    }

    // Octaves: tempos are read within PREFERRED (78–180 BPM). Where the slower reading
    // (78–90) could also be double, it is double when the snares and hi-hats sit halfway
    // between its beats (drum & bass: kick, then snare half a slow beat later) rather than
    // on them (hip-hop: snare on the beat).
    let mut bpm = best.0;
    if bpm < PREFERRED.0 && bpm * 2.0 <= MAX_BPM {
        bpm *= 2.0;
    } else if bpm > PREFERRED.1 {
        bpm /= 2.0;
    }
    let slow = if bpm >= 2.0 * PREFERRED.0 && bpm / 2.0 <= DOUBLE_BELOW {
        bpm / 2.0
    } else {
        bpm
    };
    let mut half_beat = None;
    let mut grid;
    if (PREFERRED.0..=DOUBLE_BELOW).contains(&slow) {
        grid = fit_grid(o, 60.0 * fr / slow);
        let bins = |p: f64| p * BINS as f64 / grid.period;
        let low = fold(&on.low, 0..n, grid.period);
        let high = fold(&on.high, 0..n, grid.period);
        // The slow beat is where the kick is (of the onset peak and the point halfway).
        let mut beat = bins(grid.phase);
        if fold_at(&low, beat + BINS as f64 / 2.0) > fold_at(&low, beat) {
            beat += BINS as f64 / 2.0;
        }
        let on_beat = fold_at(&high, beat).max(1e-12);
        let ratio = fold_at(&high, beat + BINS as f64 / 2.0) / on_beat;
        half_beat = Some(ratio);
        if ratio >= DOUBLE_RATIO {
            grid = fit_grid(o, grid.period / 2.0);
        }
    } else {
        grid = fit_grid(o, 60.0 * fr / bpm);
    }
    let period = grid.period;
    // A pulse repeats: the onsets one or two beats later look like these.
    let periodicity = at(&acf, period).max(at(&acf, 2.0 * period)) / acf[0].max(1e-12) as f64;

    // Beat or offbeat: of the grid's peak and the point halfway, the beat has the kick.
    let low = fold(&on.low, 0..n, period);
    let full = fold(o, 0..n, period);
    let b = grid.phase * BINS as f64 / period;
    let half = BINS as f64 / 2.0;
    let mut phase = grid.phase;
    if fold_at(&low, b + half) > 1.3 * fold_at(&low, b)
        && fold_at(&full, b + half) >= 0.5 * fold_at(&full, b)
    {
        phase = (phase + period / 2.0) % period;
    }

    let (consistency, stability) = consistency(o, period, phase);
    let (downbeat, downbeat_score) = downbeat(&on, period, phase);
    let first_ms = |frame: f64| (frame * HOP as f64 + WIN as f64 * 0.75) / rate * 1000.0;
    let bar = 4.0 * period;
    // The first downbeat where the music starts (half a beat early still counts).
    let start_frame = (start_ms as f64 / 1000.0 * rate - WIN as f64 * 0.75) / HOP as f64;
    let first_bar = ((start_frame - downbeat - period / 2.0) / bar)
        .ceil()
        .max(0.0) as usize;
    let (offset, phrase_score) = match phrase(&on.bands, period, downbeat) {
        Some((o, score)) if score >= MIN_PHRASE => (o, score),
        Some((_, score)) => (first_bar % PHRASE_BARS, score),
        None => (first_bar % PHRASE_BARS, 0.0),
    };
    let bpm = 60.0 * fr / period;
    let period_ms = 60_000.0 / bpm;
    // With log compression the flux jumps as soon as an onset enters the window, which is
    // three quarters of a window after the frame start (calibrated on synthetic clicks; the
    // bias is the same for every track, so beat matching does not depend on it).
    let anchor = downbeat + (offset * 4) as f64 * period;
    let beat_ms = first_ms(anchor).rem_euclid(period_ms * 4.0 * PHRASE_BARS as f64);
    Some(TempoDetail {
        bpm,
        beat_ms,
        periodicity,
        strength: grid.contrast,
        consistency,
        stability,
        beat_onset: grid.excess,
        coverage: coverage(o, period, phase),
        half_beat,
        downbeat: downbeat_score,
        phrase: phrase_score,
    })
}

/// Agreement of the beat phase along the track: each 4-bar stretch is folded on its own and
/// votes for its peak (on the beat or halfway between beats) with its clarity (peak above
/// average); 1 when they all agree, near 0 when the peaks wander (free tempo, drift). Also
/// how well the grid's tempo holds in each stretch (its fold peak against the sharpest of
/// nearby tempos; weighted alike): ≈ 1 for a steady beat, lower for rubato.
fn consistency(o: &[f32], period: f64, phase: f64) -> (f64, f64) {
    let seg = (16.0 * period) as usize;
    let (mut re, mut im, mut total, mut held) = (0.0, 0.0, 0.0, 0.0);
    let mut from = 0;
    while from + seg <= o.len() {
        let h = fold(o, from..from + seg, period);
        let (bin, peak) = fold_peak(&h);
        let w = peak - h.iter().sum::<f64>() / BINS as f64;
        // The stretch's own tempo: no nearby period (±8 %) folds it much sharper.
        let sharpest = (-8..=8)
            .map(|r| {
                let p = period * (1.0 + r as f64 * 0.01);
                fold_peak(&fold(o, from..from + seg, p)).1
            })
            .fold(peak, f64::max);
        held += w * peak / sharpest.max(1e-12);
        // Doubled: a stretch where the hi-hats between the beats are louder than the beats
        // still follows the grid.
        let angle = 4.0 * std::f64::consts::PI * (bin / BINS as f64 - phase / period);
        re += w * angle.cos();
        im += w * angle.sin();
        total += w;
        from += seg;
    }
    if total <= 0.0 {
        return (0.0, 0.0);
    }
    ((re * re + im * im).sqrt() / total, held / total)
}

/// Share of the beats of the grid with an onset above the average onset on them.
fn coverage(o: &[f32], period: f64, phase: f64) -> f64 {
    let mean = o.iter().map(|&v| v as f64).sum::<f64>() / o.len().max(1) as f64;
    let (mut on, mut all) = (0usize, 0usize);
    for t in beats(period, phase, o.len()) {
        let i = t.round() as usize;
        let near = o[i.saturating_sub(1)..(i + 2).min(o.len())]
            .iter()
            .fold(0f32, |m, &x| m.max(x));
        on += usize::from(near as f64 > mean);
        all += 1;
    }
    on as f64 / all.max(1) as f64
}

/// Beats of the grid (frames) within `n` frames.
fn beats(period: f64, phase: f64, n: usize) -> impl Iterator<Item = f64> {
    (0..)
        .map(move |k| phase + k as f64 * period)
        .take_while(move |&t| t < n as f64)
}

/// Mean coarse spectrum (log magnitudes per band) over frames `[from, to)`.
fn mean_bands(bands: &[[f32; BANDS]], from: f64, to: f64) -> [f64; BANDS] {
    let (lo, hi) = (
        from.max(0.0) as usize,
        (to.max(0.0) as usize).min(bands.len()),
    );
    let mut m = [0f64; BANDS];
    if hi <= lo {
        return m;
    }
    for f in &bands[lo..hi] {
        for (s, &v) in m.iter_mut().zip(f) {
            *s += v as f64;
        }
    }
    m.map(|s| s / (hi - lo) as f64)
}

/// The first downbeat (frames, within the first bar) and how much its beat of the bar
/// stands out from the four: bars start with a kick and with new bass notes and chords (the
/// spectrum below 640 Hz gains most from the beat before).
fn downbeat(on: &Onsets, period: f64, phase: f64) -> (f64, f64) {
    let n = on.full.len();
    let near = |v: &[f32], t: f64| {
        let i = t.round() as usize;
        v[i.saturating_sub(1)..(i + 2).min(v.len())]
            .iter()
            .fold(0f32, |m, &x| m.max(x)) as f64
    };
    let ts: Vec<f64> = beats(period, phase, n).collect();
    if ts.len() < 16 {
        return (phase, 1.0);
    }
    let spectra: Vec<[f64; BANDS]> = ts
        .iter()
        .map(|&t| mean_bands(&on.bands, t, t + period))
        .collect();
    let kick: Vec<f64> = ts.iter().map(|&t| near(&on.low, t)).collect();
    let change: Vec<f64> = (0..ts.len())
        .map(|k| match k {
            0 => 0.0,
            _ => (0..HARMONIC_BANDS)
                .map(|b| (spectra[k][b] - spectra[k - 1][b]).max(0.0))
                .sum(),
        })
        .collect();
    let norm = |v: &[f64]| {
        let m = v.iter().sum::<f64>() / v.len() as f64;
        if m > 0.0 { 1.0 / m } else { 0.0 }
    };
    let (nk, nc) = (norm(&kick), norm(&change));
    let mut score = [0f64; 4];
    let mut count = [0f64; 4];
    for k in 0..ts.len() {
        score[k % 4] += kick[k] * nk + change[k] * nc;
        count[k % 4] += 1.0;
    }
    let score: Vec<f64> = (0..4).map(|c| score[c] / count[c].max(1.0)).collect();
    let best = (0..4)
        .max_by(|&a, &b| score[a].total_cmp(&score[b]))
        .unwrap_or(0);
    let mean = score.iter().sum::<f64>() / 4.0;
    let clarity = if mean > 0.0 { score[best] / mean } else { 1.0 };
    (phase + best as f64 * period, clarity)
}

/// Phrases: the bar offset (`0..PHRASE_BARS` from the first downbeat) where the sound
/// changes most, every [`PHRASE_BARS`] bars, and how much it stands out from the others.
fn phrase(bands: &[[f32; BANDS]], period: f64, downbeat: f64) -> Option<(usize, f64)> {
    let bar = 4.0 * period;
    let bars: Vec<[f64; BANDS]> = beats(bar, downbeat, bands.len())
        .take_while(|&t| t + bar <= bands.len() as f64)
        .map(|t| mean_bands(bands, t, t + bar))
        .collect();
    if bars.len() < 3 * PHRASE_BARS {
        return None;
    }
    let mut score = [0f64; PHRASE_BARS];
    let mut count = [0f64; PHRASE_BARS];
    for j in 1..bars.len() {
        let change: f64 = (0..BANDS)
            .map(|b| (bars[j][b] - bars[j - 1][b]).abs())
            .sum();
        score[j % PHRASE_BARS] += change;
        count[j % PHRASE_BARS] += 1.0;
    }
    let score: Vec<f64> = (0..PHRASE_BARS)
        .map(|o| score[o] / count[o].max(1.0))
        .collect();
    let best = (0..PHRASE_BARS).max_by(|&a, &b| score[a].total_cmp(&score[b]))?;
    let mean = score.iter().sum::<f64>() / PHRASE_BARS as f64;
    (mean > 0.0).then(|| (best, score[best] / mean))
}

/// Listening bench only: the tempo estimate of a file with its steadiness measures, also
/// when [`analyze_file`] would reject the beat.
#[cfg(feature = "bench")]
#[doc(hidden)]
pub fn tempo_detail_file(path: &Path) -> Result<Option<TempoDetail>> {
    let (x, rate) = decode_decimated(path)?;
    let mut a = TrackAnalysis::empty();
    if !envelope(&x, rate, &mut a) {
        return Ok(None);
    }
    Ok(tempo_detail(&x, rate, a.start_ms))
}

/// Krumhansl–Kessler key profiles (C major, C minor).
const MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Lowest and highest note (MIDI) of the chroma: C2 (65 Hz) to B6 (1976 Hz).
const CHROMA_NOTES: (i32, i32) = (36, 95);

fn key(x: &[f32], rate: f64) -> Option<Key> {
    let chroma = chroma(x, rate)?;
    let mut best = (None, 0.3);
    for tonic in 0..12 {
        for (profile, minor) in [(&MAJOR, false), (&MINOR, true)] {
            let rotated: Vec<f64> = (0..12).map(|pc| profile[(pc + 12 - tonic) % 12]).collect();
            let r = pearson(&chroma, &rotated);
            if r > best.1 {
                best = (Some(Key::new(tonic as u8, minor)), r);
            }
        }
    }
    best.0
}

/// How much of each pitch class (C = 0) the track has.
fn chroma(x: &[f32], rate: f64) -> Option<[f64; 12]> {
    if x.len() < CHROMA_WIN {
        return None;
    }
    let mut fft = Fft::new(CHROMA_WIN);
    let hz_per_bin = rate / CHROMA_WIN as f64;
    // Each note takes its strongest bin among the same number of bins around it as the
    // other notes of its octave have (as many as the octave's C spans, ±¼ semitone or more):
    // every pitch class counts alike, although a semitone spans 1–2 bins at C2 and over 40 at
    // B6, and noise does not favour the upper ones.
    let hz = |m: f64| 440.0 * 2f64.powf((m - 69.0) / 12.0);
    let notes: Vec<(usize, usize, f64, usize)> = (CHROMA_NOTES.0..=CHROMA_NOTES.1)
        .map(|midi| {
            let c = (midi - CHROMA_NOTES.0) / 12 * 12 + CHROMA_NOTES.0;
            let w = (((hz(c as f64 + 0.5) - hz(c as f64 - 0.5)) / hz_per_bin) as usize).max(1);
            let centre = hz(midi as f64) / hz_per_bin;
            let lo = (centre - (w as f64 - 1.0) / 2.0).round().max(1.0) as usize;
            let hi = (lo + w).min(fft.len() / 2 + 1);
            (lo, hi, centre, midi.rem_euclid(12) as usize)
        })
        .collect();
    let mut frames: Vec<[f64; 12]> = Vec::new();
    let mut mag = Vec::new();
    for start in (0..=x.len() - CHROMA_WIN).step_by(CHROMA_WIN / 2) {
        fft.magnitudes(&x[start..start + CHROMA_WIN], &mut mag);
        let mut c = [0f64; 12];
        for &(lo, hi, centre, pc) in &notes {
            c[pc] += if hi - lo > 1 {
                mag[lo..hi].iter().fold(0f32, |m, &v| m.max(v)) as f64
            } else {
                // One bin per note (lowest octave): read the spectrum at the note itself, so
                // notes between two bins are not weaker.
                at(&mag, centre)
            };
        }
        frames.push(c);
    }
    // Every frame with sound counts alike (its chroma scaled to a peak of 1), so loud
    // passages do not outvote the rest; near-silent frames are left out.
    let level: Vec<f64> = frames.iter().map(|c| c.iter().sum()).collect();
    let mut sorted = level.clone();
    sorted.sort_by(f64::total_cmp);
    let typical = sorted[sorted.len() * 4 / 5];
    if typical < 1e-3 {
        return None;
    }
    let mut chroma = [0f64; 12];
    for (c, &l) in frames.iter().zip(&level) {
        let peak = c.iter().fold(0f64, |m, &v| m.max(v));
        if l < typical * 0.05 || peak <= 0.0 {
            continue;
        }
        for (s, v) in chroma.iter_mut().zip(c) {
            *s += v / peak;
        }
    }
    Some(chroma)
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        num += (x - ma) * (y - mb);
        da += (x - ma) * (x - ma);
        db += (y - mb) * (y - mb);
    }
    if da <= 0.0 || db <= 0.0 {
        return 0.0;
    }
    num / (da * db).sqrt()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::decoder::tests::fixture;
    use std::f64::consts::PI;

    pub const RATE: u32 = 44_100;

    /// A deterministic click track: decaying 1.5 kHz bursts every beat from `first_ms`, over a
    /// quiet low drone, `secs` long.
    pub fn clicks(bpm: f64, first_ms: f64, secs: f64) -> Vec<f32> {
        let n = (secs * RATE as f64) as usize;
        let period = 60.0 / bpm * RATE as f64;
        let first = first_ms / 1000.0 * RATE as f64;
        let mut x = vec![0f32; n];
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f64 / RATE as f64;
            *s = 0.05 * (2.0 * PI * 110.0 * t).sin() as f32;
        }
        let mut k = 0.0;
        loop {
            let at = (first + k * period) as usize;
            if at >= n {
                break;
            }
            for j in 0..(RATE as usize / 20).min(n - at) {
                let t = j as f64 / RATE as f64;
                x[at + j] += (0.8 * (-t * 60.0).exp() * (2.0 * PI * 1500.0 * t).sin()) as f32;
            }
            k += 1.0;
        }
        x
    }

    /// Error of the analysed grid against beats every `period` ms from `first_ms` (folded
    /// into ±period/2).
    fn phase_error(a: &TrackAnalysis, first_ms: f64, period: f64) -> f64 {
        let d = (a.beat_ms as f64 - first_ms).rem_euclid(period);
        d.min(period - d)
    }

    #[test]
    fn finds_tempo_and_beat_of_click_tracks() {
        for (bpm, first_ms) in [(120.0, 0.0), (128.0, 300.0), (97.0, 1234.0), (174.0, 50.0)] {
            let a = analyze_pcm(&clicks(bpm, first_ms, 40.0), RATE);
            // Half or double tempo is as good for beat matching (174 reads as 87).
            let ratio = a.bpm as f64 / bpm;
            let octave = [0.5, 1.0, 2.0]
                .into_iter()
                .find(|m| (ratio - m).abs() < 0.2 / bpm);
            assert!(octave.is_some(), "{bpm}: {a:?}");
            assert!(
                phase_error(&a, first_ms, 60_000.0 / bpm) < 15.0,
                "{bpm}: {a:?}"
            );
        }
    }

    #[test]
    fn grid_stays_on_the_beat_to_the_end() {
        // Over 4 minutes a tempo error of 0.05 BPM drifts by ~100 ms: the fit must be tighter.
        let bpm = 123.0;
        let a = analyze_pcm(&clicks(bpm, 200.0, 240.0), RATE);
        let p = a.beat_period_ms().unwrap();
        let k = (230_000.0 / p).floor();
        // The grid's anchor is a phrase start, a whole number of beats from the first beat.
        let predicted = a.beat_ms as f64 + k * (60_000.0 / a.bpm as f64);
        let actual = 200.0 + k * 60_000.0 / bpm;
        let d = (predicted - actual).rem_euclid(p);
        assert!(d.min(p - d) < 20.0, "{predicted} vs {actual}");
    }

    /// Sine chords, 2 s each, `secs` in total.
    fn chords(chords: &[&[f64]], secs: f64) -> Vec<f32> {
        let n = (secs * RATE as f64) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                let chord = chords[(t / 2.0) as usize % chords.len()];
                let s: f64 = chord
                    .iter()
                    .map(|&midi| (2.0 * PI * 440.0 * 2f64.powf((midi - 69.0) / 12.0) * t).sin())
                    .sum();
                (0.2 * s) as f32
            })
            .collect()
    }

    #[test]
    fn finds_major_and_minor_keys() {
        // C – F – G – C.
        let c = chords(
            &[
                &[48.0, 60.0, 64.0, 67.0],
                &[53.0, 65.0, 69.0, 72.0],
                &[55.0, 67.0, 71.0, 74.0],
                &[48.0, 60.0, 64.0, 67.0],
            ],
            16.0,
        );
        assert_eq!(analyze_pcm(&c, RATE).key, Some(Key::new(0, false)));
        // Am – Dm – E – Am.
        let am = chords(
            &[
                &[57.0, 69.0, 72.0, 76.0],
                &[50.0, 62.0, 65.0, 69.0],
                &[52.0, 64.0, 68.0, 71.0],
                &[57.0, 69.0, 72.0, 76.0],
            ],
            16.0,
        );
        assert_eq!(analyze_pcm(&am, RATE).key, Some(Key::new(9, true)));
    }

    #[test]
    fn camelot_compatibility() {
        let c = Key::new(0, false);
        assert_eq!(c.camelot(), 8);
        assert_eq!(Key::new(9, true).camelot(), 8);
        assert_eq!(Key::new(11, false).camelot(), 1);
        assert!(c.compatible(Key::new(9, true)), "relative minor");
        assert!(c.compatible(Key::new(7, false)), "G: a fifth up");
        assert!(c.compatible(Key::new(5, false)), "F: a fifth down");
        assert!(!c.compatible(Key::new(1, false)), "Db");
        assert!(!c.compatible(Key::new(4, true)), "Em is 9A, not next to 8B");
        for i in 0..24 {
            assert_eq!(Key::from_index(i).unwrap().index(), i);
        }
        assert!(Key::from_index(24).is_none());
    }

    #[test]
    fn finds_intro_outro_and_silence() {
        // 1 s silence, 4 s quiet intro, 20 s body, 6 s fade-out, 2 s silence.
        let secs = 33.0;
        let n = (secs * RATE as f64) as usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                let level = match t {
                    t if t < 1.0 => 0.0,
                    t if t < 5.0 => 0.03,
                    t if t < 25.0 => 0.5,
                    t if t < 31.0 => 0.5 * (31.0 - t) / 6.0,
                    _ => 0.0,
                };
                (level * (2.0 * PI * 330.0 * t).sin()) as f32
            })
            .collect();
        let a = analyze_pcm(&x, RATE);
        assert_eq!(a.start_ms, 1000, "{a:?}");
        assert!(a.end_ms.abs_diff(31_000) <= 500, "{a:?}");
        assert!(a.intro_end_ms.abs_diff(5_000) <= 1000, "{a:?}");
        // −6 dB below the body is reached halfway through the fade.
        assert!(a.outro_start_ms.abs_diff(28_000) <= 1500, "{a:?}");
        assert_eq!(a.bpm, 0.0, "no beat in a sine");
    }

    #[test]
    fn silence_is_unusable() {
        let a = analyze_pcm(&vec![0.0; RATE as usize * 10], RATE);
        assert!(!a.usable());
        // A steady tone with a few level changes has no beat.
        let tone: Vec<f32> = (0..RATE as usize * 20)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                let level = if (t as u32) % 7 == 3 { 0.1 } else { 0.4 };
                (level * (2.0 * PI * 220.0 * t).sin()) as f32
            })
            .collect();
        assert_eq!(analyze_pcm(&tone, RATE).bpm, 0.0);
        let a = analyze_pcm(&vec![0.0; RATE as usize * 10], RATE);
        assert!(!a.usable());
        assert_eq!(a.key, None);
    }

    #[test]
    fn analyses_a_file() {
        let a = analyze_file(&fixture("sine_opus.webm")).unwrap();
        assert!(a.usable());
        assert!(a.end_ms.abs_diff(2000) <= 250, "{a:?}");
        assert_eq!(a.bpm, 0.0);
    }

    /// Deterministic noise in −1…1.
    fn noise(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }

    /// Adds a drum hit at `at_s`: a kick (pitched 55 Hz thump), a snare (noise with a 200 Hz
    /// body) or a hi-hat (quiet bright noise).
    fn hit(x: &mut [f32], at_s: f64, kind: char, seed: &mut u64) {
        let at = (at_s * RATE as f64) as usize;
        for j in 0..(RATE as usize / 8).min(x.len().saturating_sub(at)) {
            let t = j as f64 / RATE as f64;
            let v = match kind {
                'k' => 0.9 * (-t * 25.0).exp() * (2.0 * PI * 55.0 * t).sin(),
                's' => (-t * 30.0).exp() * (0.5 * noise(seed) + 0.3 * (2.0 * PI * 200.0 * t).sin()),
                _ => 0.12 * (-t * 80.0).exp() * noise(seed),
            };
            x[at + j] += v as f32;
        }
    }

    /// A drum pattern at `bpm` for `secs`: hits per bar of 4 beats at fractional beat positions.
    fn drums(bpm: f64, secs: f64, pattern: &[(f64, char)]) -> Vec<f32> {
        let mut x = vec![0f32; (secs * RATE as f64) as usize];
        let beat = 60.0 / bpm;
        let mut seed = 7;
        let mut bar = 0.0;
        while bar * 4.0 * beat < secs {
            for &(pos, kind) in pattern {
                let t = (bar * 4.0 + pos) * beat;
                if t < secs {
                    hit(&mut x, t, kind, &mut seed);
                }
            }
            bar += 1.0;
        }
        x
    }

    fn eighth_hats() -> Vec<(f64, char)> {
        (0..8).map(|i| (i as f64 * 0.5, 'h')).collect()
    }

    #[test]
    fn reads_drum_and_bass_double_and_hip_hop_single() {
        // Drum & bass at 174: kick on 1 and the "and" of 3, snare on 2 and 4 — at 87 BPM the
        // snares fall halfway between the beats.
        let mut dnb = vec![(0.0, 'k'), (1.0, 's'), (2.5, 'k'), (3.0, 's')];
        dnb.extend(eighth_hats());
        let a = analyze_pcm(&drums(174.0, 60.0, &dnb), RATE);
        assert!((a.bpm - 174.0).abs() < 0.5, "{a:?}");
        // Hip-hop at 88: kick on 1 and 3, snare on 2 and 4 (on its beats), hi-hats in eighths.
        let mut hh = vec![(0.0, 'k'), (1.0, 's'), (2.0, 'k'), (2.75, 'k'), (3.0, 's')];
        hh.extend(eighth_hats());
        let a = analyze_pcm(&drums(88.0, 60.0, &hh), RATE);
        assert!((a.bpm - 88.0).abs() < 0.5, "{a:?}");
    }

    #[test]
    fn finds_the_downbeat_of_a_rock_beat() {
        // Kick on 1 and 3, snare on 2 and 4, eighth hi-hats, the bass changing note on every
        // 1; the music starts 300 ms in.
        let mut rock = vec![(0.0, 'k'), (1.0, 's'), (2.0, 'k'), (3.0, 's')];
        rock.extend(eighth_hats());
        let mut x = vec![0f32; (0.3 * RATE as f64) as usize];
        x.extend(drums(120.0, 60.0, &rock));
        for (i, s) in x.iter_mut().enumerate().skip((0.3 * RATE as f64) as usize) {
            let t = i as f64 / RATE as f64 - 0.3;
            let note = [40.0, 45.0, 43.0, 38.0][(t / 2.0) as usize % 4];
            let hz = 440.0 * 2f64.powf((note - 69.0) / 12.0);
            *s += (0.3 * (2.0 * PI * hz * t).sin()) as f32;
        }
        let a = analyze_pcm(&x, RATE);
        assert!((a.bpm - 120.0).abs() < 0.3, "{a:?}");
        let bar = 2000.0;
        let d = (a.beat_ms as f64 - 300.0).rem_euclid(bar);
        assert!(d.min(bar - d) < 15.0, "downbeat at {} ms: {a:?}", a.beat_ms);
    }

    #[test]
    fn phrases_start_where_the_sound_changes() {
        // Four on the floor at 125 BPM (bars of 1.92 s); a bass line changes note at bar 3
        // and every 8 bars after, so phrases start there.
        let bpm = 125.0;
        let bar = 4.0 * 60.0 / bpm;
        let mut x = drums(
            bpm,
            120.0,
            &[(0.0, 'k'), (1.0, 'k'), (2.0, 'k'), (3.0, 'k')],
        );
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f64 / RATE as f64;
            let phrase = ((t / bar - 3.0) / 8.0).floor() as i64;
            let note = [45.0, 50.0, 41.0, 43.0][phrase.rem_euclid(4) as usize];
            let hz = 440.0 * 2f64.powf((note - 69.0) / 12.0);
            *s += (0.25 * (2.0 * PI * hz * t).sin() + 0.1 * (2.0 * PI * hz * 4.0 * t).sin()) as f32;
        }
        let a = analyze_pcm(&x, RATE);
        assert!((a.bpm as f64 - bpm).abs() < 0.3, "{a:?}");
        let phrase_ms = 8.0 * bar * 1000.0;
        let d = (a.beat_ms as f64 - 3.0 * bar * 1000.0).rem_euclid(phrase_ms);
        assert!(
            d.min(phrase_ms - d) < 15.0,
            "phrase at {} ms: {a:?}",
            a.beat_ms
        );
    }

    #[test]
    fn rejects_random_onsets_and_free_tempo() {
        // Clicks at random intervals (0.15–0.85 s).
        let mut seed = 3;
        let mut x = vec![0f32; RATE as usize * 60];
        let mut t = 0.2;
        while t < 59.5 {
            hit(&mut x, t, 's', &mut seed);
            t += 0.5 + 0.35 * noise(&mut seed);
        }
        let a = analyze_pcm(&x, RATE);
        assert!(a.usable());
        assert_eq!(a.bpm, 0.0, "random: {a:?}");
        // Rubato: a beat whose tempo swells between 100 and 130 BPM every 12 s.
        let mut x = vec![0f32; RATE as usize * 90];
        let mut t = 0.2;
        while t < 89.5 {
            hit(&mut x, t, 'k', &mut seed);
            hit(&mut x, t, 'h', &mut seed);
            let bpm = 115.0 + 15.0 * (2.0 * PI * t / 12.0).sin();
            t += 60.0 / bpm;
        }
        let a = analyze_pcm(&x, RATE);
        assert_eq!(a.bpm, 0.0, "rubato: {a:?}");
    }

    #[test]
    fn finds_keys_away_from_c() {
        // D – G – A – D.
        let d = chords(
            &[
                &[50.0, 62.0, 66.0, 69.0],
                &[55.0, 67.0, 71.0, 74.0],
                &[57.0, 69.0, 73.0, 76.0],
                &[50.0, 62.0, 66.0, 69.0],
            ],
            16.0,
        );
        assert_eq!(analyze_pcm(&d, RATE).key, Some(Key::new(2, false)));
        // F#m – Bm – C# – F#m.
        let fsm = chords(
            &[
                &[54.0, 66.0, 69.0, 73.0],
                &[47.0, 59.0, 62.0, 66.0],
                &[49.0, 61.0, 65.0, 68.0],
                &[54.0, 66.0, 69.0, 73.0],
            ],
            16.0,
        );
        assert_eq!(analyze_pcm(&fsm, RATE).key, Some(Key::new(6, true)));
    }

    #[test]
    fn chroma_of_noise_is_flat() {
        // Every pitch class counts alike however many FFT bins its semitones span.
        let mut seed = 11;
        let x: Vec<f32> = (0..RATE as usize / 4 * 30)
            .map(|_| (0.3 * noise(&mut seed)) as f32)
            .collect();
        let c = chroma(&x, RATE as f64 / 4.0).unwrap();
        let (lo, hi) = c
            .iter()
            .fold((f64::MAX, 0f64), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(hi / lo < 1.1, "{c:?}");
    }
}
