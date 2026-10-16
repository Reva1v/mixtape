//! EBU R128 measurement of a whole file, run in the background after a track is stored.

use std::path::Path;

use ebur128::{EbuR128, Mode};

use crate::decoder::Decoder;
use crate::{Result, SourceOpener};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, LUFS.
    pub lufs: f64,
    /// Maximum true peak over both channels, dBTP.
    pub true_peak: f64,
}

/// Decodes the whole file (stereo, source rate) and measures it.
pub fn measure_loudness(path: &Path) -> Result<Loudness> {
    let hint = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let mut dec = Decoder::open(SourceOpener::File(path.to_path_buf()), hint.as_deref())?;
    let mut meter = EbuR128::new(2, dec.rate(), Mode::I | Mode::TRUE_PEAK)?;
    let mut pcm = Vec::new();
    loop {
        pcm.clear();
        let more = dec.next_chunk(&mut pcm)?;
        meter.add_frames_f32(&pcm)?;
        if !more {
            break;
        }
    }
    let peak = meter.true_peak(0)?.max(meter.true_peak(1)?);
    Ok(Loudness {
        lufs: meter.loudness_global()?,
        true_peak: 20.0 * peak.max(1e-10).log10(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::tests::fixture;

    #[test]
    fn measures_sine() {
        // A steady sine on both channels: loudness = RMS (peak − 3 dB) + 3 dB for two channels
        // − 0.691 dB offset of the K-weighting at ~1 kHz; 440 Hz is within ±0.3 dB of that.
        for name in ["sine_opus.webm", "sine_aac.m4a"] {
            let l = measure_loudness(&fixture(name)).unwrap();
            let expected = l.true_peak - 0.691;
            assert!((l.lufs - expected).abs() < 1.0, "{name}: {l:?}");
            assert!((-30.0..=-10.0).contains(&l.lufs), "{name}: {l:?}");
        }
    }
}
