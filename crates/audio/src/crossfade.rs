//! Equal-power crossfade of two decoded streams: the outgoing track (`a`) is mixed under the
//! incoming one (`b`) over a fixed number of frames.

use std::f64::consts::FRAC_PI_2;

use crate::gain::soft_limit;

/// Gains of the outgoing and incoming track at `t` (0…1) into the fade: cos/sin, so the summed
/// power of uncorrelated material stays constant.
pub(crate) fn curves(t: f64) -> (f32, f32) {
    let x = t.clamp(0.0, 1.0) * FRAC_PI_2;
    (x.cos() as f32, x.sin() as f32)
}

/// Decoded stereo frames of one side, not yet mixed.
#[derive(Default)]
pub(crate) struct Side {
    pub buf: Vec<f32>,
    /// Nothing more will be appended; past `buf` the side is silent.
    pub ended: bool,
}

impl Side {
    fn frames(&self) -> usize {
        self.buf.len() / 2
    }

    fn sample(&self, i: usize) -> f32 {
        self.buf.get(i).copied().unwrap_or(0.0)
    }
}

pub(crate) struct Mixer {
    len: u64,
    done: u64,
    pub a: Side,
    pub b: Side,
}

impl Mixer {
    pub fn new(len: u64) -> Self {
        Self {
            len: len.max(1),
            done: 0,
            a: Side::default(),
            b: Side::default(),
        }
    }

    /// All `len` frames are mixed (or both sides ran out).
    pub fn finished(&self) -> bool {
        let dry = |s: &Side| s.ended && s.buf.is_empty();
        self.done >= self.len || (dry(&self.a) && dry(&self.b))
    }

    /// The outgoing side must be decoded further before [`Mixer::mix`] can progress.
    pub fn needs_a(&self) -> bool {
        !self.finished() && !self.a.ended && self.a.buf.is_empty()
    }

    pub fn needs_b(&self) -> bool {
        !self.finished() && !self.b.ended && self.b.buf.is_empty()
    }

    /// Appends as many mixed frames to `out` as both sides allow.
    pub fn mix(&mut self, out: &mut Vec<f32>) {
        let mut n = (self.len - self.done.min(self.len)) as usize;
        for side in [&self.a, &self.b] {
            if !side.ended {
                n = n.min(side.frames());
            }
        }
        out.reserve(n * 2);
        for i in 0..n * 2 {
            let (ga, gb) = curves((self.done + (i / 2) as u64) as f64 / self.len as f64);
            out.push(soft_limit(self.a.sample(i) * ga + self.b.sample(i) * gb));
        }
        for side in [&mut self.a, &mut self.b] {
            side.buf.drain(..(n * 2).min(side.buf.len()));
        }
        self.done += n as u64;
    }

    /// Incoming frames decoded past the overlap; outgoing leftovers are dropped.
    pub fn into_rest(self) -> Vec<f32> {
        self.b.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_keep_constant_power() {
        assert_eq!(curves(0.0), (1.0, 0.0));
        let (a, b) = curves(1.0);
        assert!(a.abs() < 1e-6 && (b - 1.0).abs() < 1e-6);
        for i in 0..=1000 {
            let (a, b) = curves(i as f64 / 1000.0);
            assert!((a * a + b * b - 1.0).abs() < 1e-5, "power at {i}");
        }
        let (a, b) = curves(0.5);
        assert!((a - b).abs() < 1e-6, "symmetric at the midpoint");
    }

    /// Feeds `a` (constant 0.5) and `b` (constant 0.25) in uneven chunks.
    fn run(len: u64, a_frames: usize, b_frames: usize, chunk: usize) -> (Vec<f32>, Vec<f32>) {
        let mut m = Mixer::new(len);
        let (mut a_left, mut b_left) = (a_frames, b_frames);
        let mut out = Vec::new();
        while !m.finished() {
            if m.needs_a() {
                let n = a_left.min(chunk);
                m.a.buf.extend(std::iter::repeat_n(0.5, n * 2));
                a_left -= n;
                m.a.ended = a_left == 0;
            } else if m.needs_b() {
                let n = b_left.min(chunk + 7);
                m.b.buf.extend(std::iter::repeat_n(0.25, n * 2));
                b_left -= n;
                m.b.ended = b_left == 0;
            } else {
                m.mix(&mut out);
            }
        }
        (out, m.into_rest())
    }

    #[test]
    fn mixes_exactly_len_frames_in_any_chunking() {
        let (whole, rest) = run(1000, 5000, 5000, 5000);
        assert_eq!(whole.len(), 2000);
        assert_eq!(rest.len(), 2 * 4000);
        assert_eq!(whole[0], 0.5);
        let (ga, gb) = curves(0.5);
        assert!((whole[1000] - (0.5 * ga + 0.25 * gb)).abs() < 1e-6);
        let (chunked, rest) = run(1000, 5000, 5000, 97);
        assert_eq!(chunked, whole);
        assert!(rest.len() < 2 * (97 + 7));
    }

    #[test]
    fn an_ended_side_is_silent() {
        // The outgoing track runs out after 300 of 1000 frames: the incoming one keeps its curve.
        let (out, _) = run(1000, 300, 2000, 64);
        assert_eq!(out.len(), 2000);
        let (_, gb) = curves(0.8);
        assert!((out[1600] - 0.25 * gb).abs() < 1e-6);
    }
}
