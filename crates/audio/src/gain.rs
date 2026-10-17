//! Loudness normalisation gain, its click-free ramp, the user volume curve and the soft limiter
//! used when boosting.

/// Integrated loudness every track is normalised to: YouTube's long-standing playback
/// reference (and Spotify's "normal"). Loud masters are turned down to it.
pub const TARGET_LUFS: f64 = -14.0;
const MIN_GAIN_DB: f64 = -20.0;
/// Quiet tracks are lifted by at most this much, and only when their measured true peak shows
/// the boost cannot clip.
const MAX_BOOST_DB: f64 = 3.0;
/// A boost never lifts the true peak above this.
const PEAK_CEILING_DBTP: f64 = -1.0;
/// Soft limiter knee (linear, ≈ −1 dBFS).
const KNEE: f32 = 0.891;
/// Length of a gain change on a playing track (loudness arriving late, the setting toggled):
/// long enough not to click, short enough to feel immediate.
pub(crate) const GAIN_RAMP_MS: u32 = 50;

/// Gain in dB that brings a track of integrated loudness `lufs` to [`TARGET_LUFS`], like
/// YouTube does: louder tracks are turned down (at most 20 dB). A quieter track is lifted
/// only when its `true_peak` (dBTP) is known, by at most 3 dB and never above −1 dBTP, so the
/// limiter never has to act; without a peak (YouTube's value alone) it stays as it is.
/// Unknown loudness means 0 dB.
pub fn gain_for(lufs: Option<f64>, true_peak: Option<f64>) -> f32 {
    let Some(lufs) = lufs.filter(|l| l.is_finite()) else {
        return 0.0;
    };
    let gain = TARGET_LUFS - lufs;
    if gain <= 0.0 {
        return gain.max(MIN_GAIN_DB) as f32;
    }
    let headroom = true_peak
        .filter(|p| p.is_finite())
        .map_or(0.0, |peak| (PEAK_CEILING_DBTP - peak).max(0.0));
    gain.min(MAX_BOOST_DB).min(headroom) as f32
}

pub(crate) fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Per-frame linear gain moving to a new target in a straight line over a few ms. It holds
/// only numbers, so the decode path stays allocation-free.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GainRamp {
    gain: f32,
    target: f32,
    step: f32,
    left: u32,
}

impl GainRamp {
    pub fn new(gain: f32) -> Self {
        Self {
            gain,
            target: gain,
            step: 0.0,
            left: 0,
        }
    }

    /// Moves to `target` over `frames` frames (at once for 0).
    pub fn set(&mut self, target: f32, frames: u32) {
        self.target = target;
        if frames == 0 || target == self.gain {
            self.gain = target;
            self.left = 0;
        } else {
            self.step = (target - self.gain) / frames as f32;
            self.left = frames;
        }
    }

    /// The gain for the next frame.
    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.left > 0 {
            self.left -= 1;
            // The last step lands exactly on the target: unity stays bit-exact.
            self.gain = if self.left == 0 {
                self.target
            } else {
                self.gain + self.step
            };
        }
        self.gain
    }

    /// Whether the gain is above unity now or will be: then the output needs the limiter.
    pub fn boosts(&self) -> bool {
        self.gain > 1.0 || self.target > 1.0
    }
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
    fn turns_loud_tracks_down_to_the_target() {
        assert_eq!(gain_for(None, None), 0.0);
        assert_eq!(gain_for(Some(-14.0), None), 0.0);
        assert_eq!(gain_for(Some(-8.0), Some(0.5)), -6.0);
        assert_eq!(gain_for(Some(-8.0), None), -6.0);
        assert_eq!(gain_for(Some(f64::NEG_INFINITY), None), 0.0);
        assert_eq!(gain_for(Some(f64::NAN), Some(-3.0)), 0.0);
        // At most 20 dB down.
        assert_eq!(gain_for(Some(10.0), None), -20.0);
        // Cuts are never limited by peak.
        assert_eq!(gain_for(Some(-10.0), Some(1.0)), -4.0);
    }

    #[test]
    fn boosts_only_with_proven_headroom() {
        // YouTube's value alone carries no peak: no boost.
        assert_eq!(gain_for(Some(-17.0), None), 0.0);
        // Peak −10 dBTP leaves 9 dB: capped at 3.
        assert_eq!(gain_for(Some(-17.0), Some(-10.0)), 3.0);
        assert_eq!(gain_for(Some(-40.0), Some(-30.0)), 3.0);
        // Only 2 dB under the ceiling.
        assert_eq!(gain_for(Some(-20.0), Some(-3.0)), 2.0);
        // Already above the ceiling: no boost, but no extra cut either.
        assert_eq!(gain_for(Some(-20.0), Some(0.5)), 0.0);
        // A boost never exceeds what the target asks for.
        assert_eq!(gain_for(Some(-15.0), Some(-20.0)), 1.0);
    }

    #[test]
    fn ramp_moves_linearly_and_lands_exactly() {
        let mut r = GainRamp::new(1.0);
        assert_eq!(r.next(), 1.0);
        r.set(0.5, 4);
        let got: Vec<f32> = (0..6).map(|_| r.next()).collect();
        assert_eq!(got, [0.875, 0.75, 0.625, 0.5, 0.5, 0.5]);
        // Back to unity bit-exactly, whatever the rounding on the way.
        r.set(1.0, 2400);
        let mut last = 0.5;
        for _ in 0..2400 {
            let g = r.next();
            assert!(g >= last && g - last < 1e-3, "monotonic, small steps");
            last = g;
        }
        assert_eq!(last, 1.0);
        assert_eq!(r.next(), 1.0);
    }

    #[test]
    fn ramp_retargets_from_where_it_is() {
        let mut r = GainRamp::new(1.0);
        r.set(0.0, 10);
        for _ in 0..5 {
            r.next();
        }
        // Half way down, turned back up: continues from 0.5 without a jump.
        r.set(1.0, 10);
        assert!((r.next() - 0.55).abs() < 1e-6);
        let mut r = GainRamp::new(0.25);
        r.set(2.0, 0);
        assert_eq!(r.next(), 2.0);
        assert!(r.boosts());
        r.set(1.0, 100);
        assert!(r.boosts(), "still above unity while ramping down");
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
