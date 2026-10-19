//! One-time analysis of a track for smart transitions: tempo and beat grid (spectral flux,
//! autocorrelation, then a least-squares fit of the beats), key (chroma against the
//! Krumhansl–Schmuckler profiles), and where the audible part, the intro and the outro are
//! (a loudness envelope in 250 ms blocks).
//!
//! Everything runs on a mono copy decimated to ~11 kHz, so a 4-minute track takes well under a
//! second of one core; callers run it on a background thread.

use std::path::Path;

use crate::decoder::Decoder;
use crate::fft::Fft;
use crate::{Result, SourceOpener};

/// Bumped when the analysis changes enough that stored results should be redone.
pub const ANALYSIS_VERSION: u32 = 1;

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
/// Onsets on the fitted grid must be this much stronger than average for a steady beat.
const MIN_BEAT_STRENGTH: f64 = 1.5;
/// Autocorrelation at the beat period relative to lag 0 below this: no steady beat.
const MIN_PERIODICITY: f64 = 0.15;

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
    /// A beat of the (constant-tempo) grid, within the first beat period.
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
    let rate = down.rate;
    Ok(analyze_decimated(&down.out, rate))
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
    if let Some((bpm, beat_ms)) = tempo(x, rate) {
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

/// Spectral flux onset strength, one value per [`HOP`].
fn onsets(x: &[f32], rate: f64) -> Vec<f32> {
    if x.len() < WIN {
        return Vec::new();
    }
    let mut fft = Fft::new(WIN);
    let top = ((5000.0 / rate * WIN as f64) as usize).min(WIN / 2);
    let mut mag = Vec::new();
    let mut prev = vec![0f32; top];
    let mut out = Vec::with_capacity((x.len() - WIN) / HOP + 1);
    for start in (0..=x.len() - WIN).step_by(HOP) {
        fft.magnitudes(&x[start..start + WIN], &mut mag);
        let mut flux = 0.0;
        for k in 1..top {
            let m = (1.0 + 100.0 * mag[k]).ln();
            flux += (m - prev[k]).max(0.0);
            prev[k] = m;
        }
        out.push(flux);
    }
    out
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

/// Tempo (BPM) and the time of a beat (ms, within the first period), if the beat is steady.
fn tempo(x: &[f32], rate: f64) -> Option<(f64, f64)> {
    let raw = onsets(x, rate);
    let fr = rate / HOP as f64;
    let n = raw.len();
    if (n as f64) < fr * 8.0 {
        return None;
    }
    // Onsets above their local (1 s) mean.
    let w = fr as usize;
    let mut sums = vec![0f64; n + 1];
    for i in 0..n {
        sums[i + 1] = sums[i] + raw[i] as f64;
    }
    let o: Vec<f32> = (0..n)
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(w / 2), (i + w / 2 + 1).min(n));
            let mean = (sums[hi] - sums[lo]) / (hi - lo) as f64;
            (raw[i] as f64 - mean).max(0.0) as f32
        })
        .collect();
    let mean = o.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    if mean <= 0.0 {
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
    // A steady beat repeats: its lag must keep a good part of the zero-lag energy.
    let periodicity = at(&acf, 60.0 * fr / best.0) / acf[0].max(1e-12) as f64;
    if periodicity < MIN_PERIODICITY {
        return None;
    }
    let mut period = 60.0 * fr / best.0;

    // Phase of the grid, then a least-squares fit of the onset peaks near each beat.
    let mut phase = 0.0;
    let mut best_sum = -1.0;
    let mut p = 0.0;
    while p < period {
        let s: f64 = (0..)
            .map(|k| p + k as f64 * period)
            .take_while(|&t| t < n as f64)
            .map(|t| at(&o, t))
            .sum();
        if s > best_sum {
            (phase, best_sum) = (p, s);
        }
        p += 0.5;
    }
    for _ in 0..3 {
        let mut pts = Vec::new();
        let mut k = 0usize;
        loop {
            let c = phase + k as f64 * period;
            if c >= n as f64 {
                break;
            }
            let lo = (c - 0.2 * period).max(1.0) as usize;
            let hi = ((c + 0.2 * period) as usize).min(n - 2);
            if lo <= hi {
                let i = (lo..=hi)
                    .max_by(|&a, &b| o[a].total_cmp(&o[b]))
                    .expect("non-empty");
                if o[i] as f64 > mean {
                    let (y0, y1, y2) = (o[i - 1] as f64, o[i] as f64, o[i + 1] as f64);
                    let d = y0 - 2.0 * y1 + y2;
                    let off = if d.abs() > 1e-12 {
                        (0.5 * (y0 - y2) / d).clamp(-0.5, 0.5)
                    } else {
                        0.0
                    };
                    pts.push((k as f64, i as f64 + off));
                }
            }
            k += 1;
        }
        if pts.len() < 8 {
            break;
        }
        let m = pts.len() as f64;
        let (sk, st) = pts.iter().fold((0.0, 0.0), |(a, b), (k, t)| (a + k, b + t));
        let (mk, mt) = (sk / m, st / m);
        let (num, den) = pts.iter().fold((0.0, 0.0), |(a, b), (k, t)| {
            (a + (k - mk) * (t - mt), b + (k - mk) * (k - mk))
        });
        if den <= 0.0 {
            break;
        }
        let fitted = num / den;
        if !(60.0 * fr / MAX_BPM * 0.9..=60.0 * fr / MIN_BPM * 1.1).contains(&fitted) {
            break;
        }
        period = fitted;
        phase = mt - period * mk;
    }
    phase = phase.rem_euclid(period);

    let beats: Vec<f64> = (0..)
        .map(|k| phase + k as f64 * period)
        .take_while(|&t| t < n as f64)
        .map(|t| at(&o, t))
        .collect();
    let strength = beats.iter().sum::<f64>() / beats.len().max(1) as f64 / mean;
    if strength < MIN_BEAT_STRENGTH {
        return None;
    }
    let bpm = 60.0 * fr / period;
    let period_ms = 60_000.0 / bpm;
    // With log compression the flux jumps as soon as an onset enters the window, which is
    // three quarters of a window after the frame start (calibrated on synthetic clicks; the
    // bias is the same for every track, so beat matching does not depend on it).
    let beat_ms = ((phase * HOP as f64 + WIN as f64 * 0.75) / rate * 1000.0).rem_euclid(period_ms);
    Some((bpm, beat_ms))
}

/// Krumhansl–Schmuckler key profiles (C major, C minor).
const MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

fn key(x: &[f32], rate: f64) -> Option<Key> {
    if x.len() < CHROMA_WIN {
        return None;
    }
    let mut fft = Fft::new(CHROMA_WIN);
    // Bins between ~A1 and ~C7 by pitch class.
    let bins: Vec<(usize, usize)> = (1..=fft.len() / 2)
        .filter_map(|k| {
            let f = k as f64 * rate / CHROMA_WIN as f64;
            if !(52.0..=2100.0).contains(&f) {
                return None;
            }
            let midi = 69.0 + 12.0 * (f / 440.0).log2();
            Some((k, (midi.round() as i64).rem_euclid(12) as usize))
        })
        .collect();
    let mut chroma = [0f64; 12];
    let mut mag = Vec::new();
    for start in (0..=x.len() - CHROMA_WIN).step_by(CHROMA_WIN / 2) {
        fft.magnitudes(&x[start..start + CHROMA_WIN], &mut mag);
        for &(k, pc) in &bins {
            chroma[pc] += mag[k] as f64;
        }
    }
    if chroma.iter().sum::<f64>() < 1e-3 {
        return None;
    }
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
        let predicted = a.beat_ms as f64 + k * (60_000.0 / a.bpm as f64);
        let actual = 200.0 + k * 60_000.0 / bpm;
        assert!((predicted - actual).abs() < 20.0, "{predicted} vs {actual}");
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
}
