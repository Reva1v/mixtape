//! Loudness normalisation gain, user volume curve and the soft limiter used when boosting.

/// Integrated loudness every track is normalised to.
pub const TARGET_LUFS: f64 = -14.0;
const MIN_GAIN_DB: f64 = -20.0;
const MAX_GAIN_DB: f64 = 6.0;
/// A boost never lifts the true peak above this.
const PEAK_CEILING_DBTP: f64 = -1.0;
/// Soft limiter knee (linear, ≈ −1 dBFS).
const KNEE: f32 = 0.891;

/// Gain in dB that brings a track to −14 LUFS, clamped to −20…+6 dB. A boost is reduced so the
/// true peak stays at or below −1 dBTP. Unknown loudness means 0 dB.
pub fn gain_for(lufs: Option<f64>, true_peak: Option<f64>) -> f32 {
    let Some(lufs) = lufs.filter(|l| l.is_finite()) else {
        return 0.0;
    };
    let mut gain = (TARGET_LUFS - lufs).clamp(MIN_GAIN_DB, MAX_GAIN_DB);
    if let Some(peak) = true_peak.filter(|p| p.is_finite())
        && gain > 0.0
    {
        gain = gain.min((PEAK_CEILING_DBTP - peak).max(0.0));
    }
    gain as f32
}

pub(crate) fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Maps a 0…1 slider to amplitude; cubic is close to perceived loudness over ~60 dB.
pub(crate) fn volume_curve(v: f32) -> f32 {
    let v = if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        1.0
    };
    v * v * v
}

/// Transparent below the knee, then bends smoothly towards ±1 instead of clipping.
#[inline]
pub(crate) fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let room = 1.0 - KNEE;
    (KNEE + room * ((a - KNEE) / room).tanh()).copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_to_target() {
        assert_eq!(gain_for(None, None), 0.0);
        assert_eq!(gain_for(Some(-14.0), None), 0.0);
        assert_eq!(gain_for(Some(-8.0), Some(0.5)), -6.0);
        assert_eq!(gain_for(Some(-17.0), None), 3.0);
        assert_eq!(gain_for(Some(f64::NEG_INFINITY), None), 0.0);
    }

    #[test]
    fn clamps_and_respects_peak() {
        assert_eq!(gain_for(Some(10.0), None), -20.0);
        assert_eq!(gain_for(Some(-40.0), None), 6.0);
        // Boost of 6 dB would push −4 dBTP to +2: limited to 3 dB.
        assert_eq!(gain_for(Some(-20.0), Some(-4.0)), 3.0);
        // Already above the ceiling: no boost, but no extra cut either.
        assert_eq!(gain_for(Some(-20.0), Some(0.5)), 0.0);
        // Cuts are never limited by peak.
        assert_eq!(gain_for(Some(-10.0), Some(1.0)), -4.0);
    }

    #[test]
    fn curves() {
        assert!((db_to_linear(-6.0) - 0.501).abs() < 0.001);
        assert_eq!(volume_curve(1.0), 1.0);
        assert_eq!(volume_curve(0.0), 0.0);
        assert_eq!(volume_curve(2.0), 1.0);
        assert_eq!(soft_limit(0.5), 0.5);
        assert!(soft_limit(3.0) <= 1.0 && soft_limit(3.0) > 0.99);
        assert!(soft_limit(-0.95) > -0.95 && soft_limit(-0.95) < -KNEE);
    }
}
