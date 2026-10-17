//! One source on its way to the ring: decode → stereo → resample to the device rate → gain.

use crate::decoder::Decoder;
use crate::gain::{GAIN_RAMP_MS, GainRamp, db_to_linear, soft_limit};
use crate::resample::Resampler;
use crate::{Result, Source};

/// Fade length on load/seek, so a cut into the middle of a waveform does not click.
pub(crate) const FADE_MS: u32 = 10;

pub(crate) struct Pipeline {
    pub id: String,
    /// Engine-assigned identity (ids may repeat, e.g. repeat-one).
    pub seq: u64,
    /// Whether the transition into this source may crossfade.
    pub crossfade: bool,
    dec: Decoder,
    resampler: Option<Resampler>,
    out_rate: u32,
    /// Output-rate frame of the next emitted frame, counted from the start of the track.
    out_pos: u64,
    /// The track's normalisation gain, applied while `normalize` is on.
    gain_db: f32,
    normalize: bool,
    /// Linear gain per frame, ramping on changes; above unity the soft limiter acts.
    gain: GainRamp,
    fade_len: u32,
    /// Frames left in the current fade-in.
    fade_left: u32,
    scratch: Vec<f32>,
    /// Something was emitted: gain changes glide from now on instead of applying at once.
    emitted: bool,
    done: bool,
}

impl Pipeline {
    pub fn open(src: Source, out_rate: u32) -> Result<Self> {
        let dec = Decoder::open(src.open, src.hint_ext.as_deref())?;
        let mut p = Self {
            id: src.id,
            seq: 0,
            crossfade: src.crossfade,
            dec,
            resampler: None,
            out_rate,
            out_pos: 0,
            gain_db: src.gain_db,
            normalize: true,
            gain: GainRamp::new(db_to_linear(src.gain_db)),
            fade_len: 0,
            fade_left: 0,
            scratch: Vec::new(),
            emitted: false,
            done: false,
        };
        p.rebuild_resampler()?;
        Ok(p)
    }

    /// Retargets the output rate; a no-op when it is unchanged (re-priming on every seek).
    pub fn set_out_rate(&mut self, rate: u32) -> Result<()> {
        if rate == self.out_rate {
            return Ok(());
        }
        self.out_rate = rate;
        self.rebuild_resampler()
    }

    fn rebuild_resampler(&mut self) -> Result<()> {
        let rate = self.out_rate;
        self.fade_len = rate * FADE_MS / 1000;
        self.resampler = match self.dec.rate() {
            r if r == rate => None,
            r => Some(Resampler::new(r, rate)?),
        };
        Ok(())
    }

    /// Changes the track's normalisation gain; `ramp` glides there (a playing track), else it
    /// applies at once (nothing emitted yet).
    pub fn set_gain_db(&mut self, db: f32, ramp: bool) {
        self.gain_db = db;
        self.retarget(ramp);
    }

    /// Turns normalisation on or off (unity gain), see [`Pipeline::set_gain_db`].
    pub fn set_normalize(&mut self, on: bool, ramp: bool) {
        self.normalize = on;
        self.retarget(ramp);
    }

    fn retarget(&mut self, ramp: bool) {
        let target = if self.normalize {
            db_to_linear(self.gain_db)
        } else {
            1.0
        };
        let frames = if ramp && self.emitted {
            self.out_rate * GAIN_RAMP_MS / 1000
        } else {
            0
        };
        self.gain.set(target, frames);
    }

    /// Starts the next emitted audio with a short fade-in.
    pub fn fade_in(&mut self) {
        self.fade_left = self.fade_len;
    }

    /// Decodes one packet's worth and appends stereo frames at the output rate (possibly none).
    /// Returns `false` once everything has been emitted.
    pub fn fill(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        if self.done {
            return Ok(false);
        }
        let start = out.len();
        self.scratch.clear();
        let more = self.dec.next_chunk(&mut self.scratch)?;
        if self
            .resampler
            .as_ref()
            .is_some_and(|r| r.rates().0 != self.dec.rate())
        {
            // The codec revised its rate after the first frame (e.g. implicit SBR).
            self.rebuild_resampler()?;
        }
        match &mut self.resampler {
            Some(rs) => {
                rs.process(&self.scratch, out)?;
                if !more {
                    rs.finish(out)?;
                }
            }
            None => out.extend_from_slice(&self.scratch),
        }
        self.apply_gain(&mut out[start..]);
        self.emitted |= out.len() > start;
        self.out_pos += ((out.len() - start) / 2) as u64;
        self.done = !more;
        Ok(more)
    }

    pub fn seek(&mut self, ms: u64) -> Result<()> {
        self.dec.seek(ms)?;
        if let Some(rs) = &mut self.resampler {
            rs.reset();
        }
        self.done = false;
        self.out_pos = self.dec.position() * self.out_rate as u64 / self.dec.rate() as u64;
        self.fade_in();
        Ok(())
    }

    /// Output-rate position of the next emitted frame.
    pub fn out_pos(&self) -> u64 {
        self.out_pos
    }

    /// Counts the length of a local file whose container does not state it (fragmented MP4).
    pub fn measure(&mut self) {
        self.dec.measure();
    }

    /// Track length in output-rate frames, when known.
    pub fn out_len(&self) -> Option<u64> {
        self.dec
            .duration_frames()
            .map(|f| f * self.out_rate as u64 / self.dec.rate() as u64)
    }

    /// Decode head position in ms (what has been produced, not what is audible).
    pub fn position_ms(&self) -> u64 {
        self.dec.position() * 1000 / self.dec.rate() as u64
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.dec
            .duration_frames()
            .map(|f| f * 1000 / self.dec.rate() as u64)
    }

    fn apply_gain(&mut self, samples: &mut [f32]) {
        let limit = self.gain.boosts();
        for frame in samples.chunks_exact_mut(2) {
            let mut g = self.gain.next();
            if self.fade_left > 0 {
                g *= 1.0 - self.fade_left as f32 / self.fade_len as f32;
                self.fade_left -= 1;
            }
            for s in frame {
                *s *= g;
                if limit {
                    *s = soft_limit(*s);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::tests::fixture;

    fn open(name: &str, out_rate: u32) -> Pipeline {
        Pipeline::open(Source::file(name, fixture(name), 0.0), out_rate).unwrap()
    }

    fn drain(p: &mut Pipeline, out: &mut Vec<f32>) {
        while p.fill(out).unwrap() {}
    }

    /// Longest run of near-silent frames in `pcm[from..to]` (frames).
    fn longest_silence(pcm: &[f32], from: usize, to: usize) -> usize {
        let (mut best, mut run) = (0, 0);
        for f in pcm[from * 2..to * 2].chunks_exact(2) {
            run = if f[0].abs() < 0.01 { run + 1 } else { 0 };
            best = best.max(run);
        }
        best
    }

    /// A then B through the same path the engine uses must give len(A) + len(B) frames at the
    /// output rate with nothing inserted at the boundary. Tolerances: WebM block durations are
    /// in whole ms (±1 ms of the trimmed Opus tail); fragmented AAC without an edit-list duration
    /// keeps its last frame's encoder padding (< 1024 source frames).
    fn check_gapless(a: &str, b: &str, out_rate: u32, tolerance_ms: u64) {
        let mut pcm = Vec::new();
        drain(&mut open(a, out_rate), &mut pcm);
        let boundary = pcm.len() / 2;
        drain(&mut open(b, out_rate), &mut pcm);
        let total = pcm.len() / 2;

        let expect = 3 * out_rate as usize;
        let diff_ms = total.abs_diff(expect) as u64 * 1000 / out_rate as u64;
        assert!(
            diff_ms <= tolerance_ms,
            "{a}+{b}: {total} frames, expected {expect}"
        );
        let a_ms = boundary as u64 * 1000 / out_rate as u64;
        assert!(a_ms.abs_diff(2000) <= tolerance_ms, "{a}: {a_ms} ms");

        // A sine crosses zero briefly; anything near-silent longer than ~2 ms at the boundary
        // would be a gap (or padding beyond the tolerance).
        let w = out_rate as usize / 20;
        let gap = longest_silence(&pcm, boundary - w, boundary + w);
        let max_gap = out_rate as usize * (2 + tolerance_ms as usize) / 1000;
        assert!(
            gap <= max_gap,
            "{a}+{b}: {gap} silent frames at the boundary"
        );
    }

    #[test]
    fn gapless_opus() {
        check_gapless("sine_opus.webm", "sine2_opus.webm", 48_000, 1);
    }

    #[test]
    fn gapless_aac_resampled() {
        check_gapless("sine_aac.m4a", "sine2_aac.m4a", 48_000, 24);
    }

    #[test]
    fn mixed_codecs_at_44k1() {
        check_gapless("sine_opus.webm", "sine2_aac.m4a", 44_100, 24);
    }

    #[test]
    fn fades_in_after_seek_and_applies_gain() {
        let peak = |gain_db: f32| {
            let src = Source::file("a", fixture("sine_opus.webm"), gain_db);
            let mut p = Pipeline::open(src, 48_000).unwrap();
            p.seek(500).unwrap();
            let mut pcm = Vec::new();
            while pcm.len() < 4800 && p.fill(&mut pcm).unwrap() {}
            assert!(pcm[0].abs() < 0.005, "fade starts at zero: {}", pcm[0]);
            assert_eq!(p.position_ms(), 500 + pcm.len() as u64 / 2 * 1000 / 48_000);
            pcm[960..].iter().fold(0f32, |m, s| m.max(s.abs()))
        };
        let unity = peak(0.0);
        assert!((peak(-6.0) / unity - 0.501).abs() < 0.01);
    }

    /// Fills `p` until `out` holds at least `frames` frames.
    fn fill_to(p: &mut Pipeline, out: &mut Vec<f32>, frames: usize) {
        while out.len() < frames * 2 && p.fill(out).unwrap() {}
    }

    #[test]
    fn gain_changes_ramp_without_a_jump() {
        const RATE: u32 = 48_000;
        let mut reference = Vec::new();
        let mut pcm = Vec::new();
        let mut p = open("sine_opus.webm", RATE);
        fill_to(&mut p, &mut pcm, 4800);
        let at = pcm.len() / 2;
        p.set_gain_db(-6.0, true);
        fill_to(&mut p, &mut pcm, 24_000);
        let off_at = pcm.len() / 2;
        // Off: unity at once, and a new track gain while off changes nothing.
        p.set_normalize(false, false);
        p.set_gain_db(-12.0, true);
        fill_to(&mut p, &mut pcm, 36_000);
        fill_to(&mut open("sine_opus.webm", RATE), &mut reference, pcm.len() / 2);

        let ramp = (RATE * GAIN_RAMP_MS / 1000) as usize;
        let target = db_to_linear(-6.0);
        for f in 0..pcm.len() / 2 {
            let x = reference[2 * f];
            // The ratio is only meaningful away from zero crossings.
            if x.abs() < 0.05 {
                continue;
            }
            let expect = match f {
                f if f < at || f >= off_at => 1.0,
                f if f >= at + ramp => target,
                f => 1.0 + (target - 1.0) * (f - at + 1) as f32 / ramp as f32,
            };
            let got = pcm[2 * f] / x;
            assert!((got - expect).abs() < 1e-3, "frame {f}: gain {got}, expected {expect}");
        }
        assert_eq!(pcm[off_at * 2..], reference[off_at * 2..pcm.len()], "unity is bit-exact");
    }
}
