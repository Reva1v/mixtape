//! A small radix-2 FFT for the track analysis (magnitude spectra of Hann-windowed frames).

use std::f64::consts::PI;

pub(crate) struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Fft {
    /// `n` must be a power of two.
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let rev = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let (cos, sin) = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .unzip();
        let window = (0..n)
            .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32)
            .collect();
        Self {
            n,
            cos,
            sin,
            rev,
            window,
            re: vec![0.0; n],
            im: vec![0.0; n],
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    /// Magnitudes of bins `0..=n/2` of the windowed frame `x` (`n` samples) into `mag`.
    pub fn magnitudes(&mut self, x: &[f32], mag: &mut Vec<f32>) {
        let n = self.n;
        for ((&s, &w), &j) in x[..n].iter().zip(&self.window).zip(&self.rev) {
            self.re[j] = s * w;
            self.im[j] = 0.0;
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let (c, s) = (self.cos[k * step], self.sin[k * step]);
                    let (i, j) = (start + k, start + k + half);
                    let tr = self.re[j] * c - self.im[j] * s;
                    let ti = self.re[j] * s + self.im[j] * c;
                    self.re[j] = self.re[i] - tr;
                    self.im[j] = self.im[i] - ti;
                    self.re[i] += tr;
                    self.im[i] += ti;
                }
            }
            size *= 2;
        }
        mag.clear();
        mag.extend((0..=n / 2).map(|k| self.re[k].hypot(self.im[k])));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_lands_in_its_bin() {
        let n = 1024;
        let mut fft = Fft::new(n);
        let x: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * 64.0 * i as f64 / n as f64).sin() as f32)
            .collect();
        let mut mag = Vec::new();
        fft.magnitudes(&x, &mut mag);
        assert_eq!(mag.len(), n / 2 + 1);
        let peak = (0..mag.len())
            .max_by(|&a, &b| mag[a].total_cmp(&mag[b]))
            .unwrap();
        assert_eq!(peak, 64);
        // Hann window: amplitude n/4 at the bin.
        assert!((mag[64] - n as f32 / 4.0).abs() < 1.0, "{}", mag[64]);
        assert!(mag[100] < 1e-2);
    }
}
