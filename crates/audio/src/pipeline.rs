//! One source on its way to the ring: decode → stereo → resample to the device rate → gain.

use crate::decoder::Decoder;
use crate::gain::{db_to_linear, soft_limit};
use crate::resample::Resampler;
use crate::{Result, Source};

/// Fade length on load/seek, so a cut into the middle of a waveform does not click.
pub(crate) const FADE_MS: u32 = 10;

pub(crate) struct Pipeline {
    pub id: String,
    /// Engine-assigned identity (ids may repeat, e.g. repeat-one).
    pub seq: u64,
    dec: Decoder,
    resampler: Option<Resampler>,
    out_rate: u32,
    gain: f32,
    /// Boosted tracks go through the soft limiter.
    limit: bool,
    fade_len: u32,
    /// Frames left in the current fade-in.
    fade_left: u32,
    scratch: Vec<f32>,
    done: bool,
}

impl Pipeline {
    pub fn open(src: Source, out_rate: u32) -> Result<Self> {
        let dec = Decoder::open(src.open, src.hint_ext.as_deref())?;
        let mut p = Self {
            id: src.id,
            seq: 0,
            dec,
            resampler: None,
            out_rate,
            gain: db_to_linear(src.gain_db),
            limit: src.gain_db > 0.0,
            fade_len: 0,
            fade_left: 0,
            scratch: Vec::new(),
            done: false,
        };
        p.set_out_rate(out_rate)?;
        Ok(p)
    }

    pub fn set_out_rate(&mut self, rate: u32) -> Result<()> {
        self.out_rate = rate;
        self.fade_len = rate * FADE_MS / 1000;
        self.resampler = match self.dec.rate() {
            r if r == rate => None,
            r => Some(Resampler::new(r, rate)?),
        };
        Ok(())
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
            self.set_out_rate(self.out_rate)?;
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
        self.done = !more;
        Ok(more)
    }

    pub fn seek(&mut self, ms: u64) -> Result<()> {
        self.dec.seek(ms)?;
        if let Some(rs) = &mut self.resampler {
            rs.reset();
        }
        self.done = false;
        self.fade_in();
        Ok(())
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
        for frame in samples.chunks_exact_mut(2) {
            let mut g = self.gain;
            if self.fade_left > 0 {
                g *= 1.0 - self.fade_left as f32 / self.fade_len as f32;
                self.fade_left -= 1;
            }
            for s in frame {
                *s *= g;
                if self.limit {
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
}
