//! The loudness YouTube Music normalises a track with, from the `player` answer's
//! `playerConfig.audioConfig`: `trackAbsoluteLoudnessLkfs` is the integrated loudness (LUFS,
//! it matches an EBU R128 measurement of the downloaded audio), `loudnessDb` the same relative
//! to `loudnessTargetLkfs`. The target is the web player's and changes over time (−7 in
//! 2026), so the absolute value is what gets stored. yt-dlp does not pass any of it through.
//!
//! The call is metadata only: without a proof-of-origin token the answer carries no usable
//! stream, but the signed-in `WEB_REMIX` client still gets the player config (a guest gets
//! UNPLAYABLE and no config, hence `None`).

use crate::Result;
use crate::innertube::InnerTube;
use mixtape_core::TrackId;
use serde_json::{Value, json};

/// Values outside this range are not loudness measurements.
const PLAUSIBLE_LUFS: std::ops::RangeInclusive<f64> = -70.0..=10.0;

/// Integrated loudness in LUFS: `trackAbsoluteLoudnessLkfs`, else `loudnessDb` (of the player
/// config or an audio format) plus `loudnessTargetLkfs`.
pub fn parse_loudness(player: &Value) -> Option<f64> {
    let num = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|x| x.is_finite());
    let plausible = |lufs: f64| PLAUSIBLE_LUFS.contains(&lufs).then_some(lufs);
    let config = player.pointer("/playerConfig/audioConfig");
    if let Some(lufs) = num(config.and_then(|c| c.get("trackAbsoluteLoudnessLkfs"))) {
        return plausible(lufs);
    }
    let target = num(config.and_then(|c| c.get("loudnessTargetLkfs")))?;
    let relative = num(config.and_then(|c| c.get("loudnessDb"))).or_else(|| {
        player
            .pointer("/streamingData/adaptiveFormats")?
            .as_array()?
            .iter()
            .filter(|f| {
                f.get("mimeType")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.starts_with("audio/"))
            })
            .find_map(|f| num(f.get("loudnessDb")))
    })?;
    plausible(relative + target)
}

impl InnerTube {
    /// YouTube's integrated loudness of `id` (LUFS), or `None` when the answer has none (a
    /// guest session, an unavailable video).
    pub async fn loudness(&self, id: &TrackId) -> Result<Option<f64>> {
        Ok(parse_loudness(&self.player_meta(id).await?))
    }

    /// The raw `player` answer for `id`; no stream is requested or used.
    pub async fn player_meta(&self, id: &TrackId) -> Result<Value> {
        self.call("player", json!({ "videoId": id.as_str() })).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signed-in `WEB_REMIX` answer, trimmed to the fields read here.
    const PLAYER: &str = include_str!("../tests/fixtures/player_loudness.json");

    #[test]
    fn reads_absolute_loudness() {
        let v: Value = serde_json::from_str(PLAYER).unwrap();
        assert_eq!(parse_loudness(&v), Some(-8.86));
    }

    #[test]
    fn falls_back_to_relative_plus_target() {
        let v = json!({ "playerConfig": { "audioConfig": { "loudnessDb": -1.86, "loudnessTargetLkfs": -7 } } });
        assert!((parse_loudness(&v).unwrap() - -8.86).abs() < 1e-9);
        let v = json!({
            "playerConfig": { "audioConfig": { "loudnessTargetLkfs": -14 } },
            "streamingData": { "adaptiveFormats": [
                { "itag": 137, "mimeType": "video/mp4; codecs=\"avc1\"", "loudnessDb": 9.0 },
                { "itag": 251, "mimeType": "audio/webm; codecs=\"opus\"", "loudnessDb": 2.5 },
            ]},
        });
        assert_eq!(parse_loudness(&v), Some(-11.5));
        // A relative value without its target means nothing.
        let v = json!({ "playerConfig": { "audioConfig": { "loudnessDb": -1.86 } } });
        assert_eq!(parse_loudness(&v), None);
    }

    #[test]
    fn missing_or_bogus_is_none() {
        assert_eq!(
            parse_loudness(&json!({ "playabilityStatus": { "status": "UNPLAYABLE" } })),
            None
        );
        let bogus =
            json!({ "playerConfig": { "audioConfig": { "trackAbsoluteLoudnessLkfs": "loud" } } });
        assert_eq!(parse_loudness(&bogus), None);
        let huge =
            json!({ "playerConfig": { "audioConfig": { "trackAbsoluteLoudnessLkfs": 1e9 } } });
        assert_eq!(parse_loudness(&huge), None);
    }
}
