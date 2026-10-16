//! Fixed-ratio stereo resampling (rubato FFT) with delay compensation, so a resampled track
//! has exactly `len · out / in` frames and lines up sample-accurately with its neighbours.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler as _};

use crate::{Error, Result};

const CHANNELS: usize = 2;
const CHUNK: usize = 1024;

pub(crate) struct Resampler {
    inner: Fft<f32>,
    from: u32,
    to: u32,
    /// Interleaved input not yet consumed by a full chunk.
    pending: Vec<f32>,
    out: Vec<f32>,
    /// Output frames still to drop to compensate the filter delay.
    skip: usize,
    fed: u64,
    produced: u64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Result<Self> {
        let inner = Fft::<f32>::new(
            from as usize,
            to as usize,
            CHUNK,
            CHANNELS,
            FixedSync::Input,
        )
        .map_err(|e| Error::Resample(e.to_string()))?;
        let skip = inner.output_delay();
        let out = vec![0.0; inner.output_frames_max() * CHANNELS];
        Ok(Self {
            inner,
            from,
            to,
            pending: Vec::new(),
            out,
            skip,
            fed: 0,
            produced: 0,
        })
    }

    pub fn rates(&self) -> (u32, u32) {
        (self.from, self.to)
    }

    /// Resamples interleaved stereo `input`, appending whatever output is ready.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.pending.extend_from_slice(input);
        self.fed += (input.len() / CHANNELS) as u64;
        while self.pending.len() / CHANNELS >= self.inner.input_frames_next() {
            self.run(None, out)?;
        }
        Ok(())
    }

    /// Emits the remaining output for everything fed so far and resets for the next stream.
    pub fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let expected = (self.fed * self.to as u64).div_ceil(self.from as u64);
        while self.produced < expected {
            let left = self.pending.len() / CHANNELS;
            let before = out.len();
            self.run(Some(left), out)?;
            let over = (self.produced.saturating_sub(expected)) as usize;
            out.truncate(out.len() - over.min((out.len() - before) / CHANNELS) * CHANNELS);
            self.produced = self.produced.min(expected);
        }
        self.reset();
        Ok(())
    }

    /// Drops buffered audio (seek).
    pub fn reset(&mut self) {
        self.inner.reset();
        self.pending.clear();
        self.skip = self.inner.output_delay();
        self.fed = 0;
        self.produced = 0;
    }

    fn run(&mut self, partial: Option<usize>, out: &mut Vec<f32>) -> Result<()> {
        let err = |e: &dyn std::fmt::Display| Error::Resample(e.to_string());
        let available = self.pending.len() / CHANNELS;
        let indexing = Indexing {
            partial_len: partial,
            ..Indexing::default()
        };
        let input =
            InterleavedSlice::new(&self.pending[..], CHANNELS, available).map_err(|e| err(&e))?;
        let frames_out = self.out.len() / CHANNELS;
        let mut output = InterleavedSlice::new_mut(&mut self.out[..], CHANNELS, frames_out)
            .map_err(|e| err(&e))?;
        let (used, made) = self
            .inner
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|e| err(&e))?;
        self.pending.drain(..used.min(available) * CHANNELS);
        let drop = self.skip.min(made);
        self.skip -= drop;
        out.extend_from_slice(&self.out[drop * CHANNELS..made * CHANNELS]);
        self.produced += (made - drop) as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, frames: usize, freq: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin() * 0.5;
                [s, s]
            })
            .collect()
    }

    #[test]
    fn exact_length_and_aligned() {
        let mut rs = Resampler::new(44_100, 48_000).unwrap();
        let input = sine(44_100, 44_100, 441.0);
        let mut out = Vec::new();
        for chunk in input.chunks(2 * 777) {
            rs.process(chunk, &mut out).unwrap();
        }
        rs.finish(&mut out).unwrap();
        assert_eq!(out.len() / 2, 48_000);
        // Delay compensated: output follows the same sine from t = 0.
        let want = sine(48_000, 48_000, 441.0);
        let err = out[2000..90_000]
            .iter()
            .zip(&want[2000..90_000])
            .map(|(a, b)| (a - b).abs());
        assert!(err.fold(0f32, f32::max) < 0.01);

        // Reusable after finish.
        let mut again = Vec::new();
        rs.process(&input[..2 * 1000], &mut again).unwrap();
        rs.finish(&mut again).unwrap();
        assert_eq!(again.len() / 2, 1089);
    }
}
