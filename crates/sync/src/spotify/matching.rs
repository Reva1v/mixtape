//! Picking the YouTube Music song that is the same recording as a Spotify track.
//!
//! A candidate is scored by its cleaned title (no «- Remastered 2011», «(feat. …)» or
//! brackets), artist overlap and duration (within ±3 s is a strong signal); audio tracks
//! beat music videos, and a version the source does not have (live, remix, acoustic…)
//! costs a lot. Below [`THRESHOLD`] the track counts as not found.

use super::SourceTrack;
use mixtape_library::normalize::normalize;
use mixtape_yt::api::{VideoKind, YtTrack};
use std::collections::HashSet;

/// Lowest score that still counts as a match.
pub const THRESHOLD: f32 = 0.6;

/// Words that mark a different version of a song; a candidate with one the source title
/// lacks (or the other way round) is another recording.
const VERSION_WORDS: &[&str] = &[
    "live",
    "remix",
    "acoustic",
    "instrumental",
    "karaoke",
    "cover",
    "sped",
    "slowed",
    "nightcore",
    "reverb",
    "demo",
    "unplugged",
    "orchestral",
    "piano",
    "mashup",
    "8d",
    "rerecorded",
    "лайв",
    "ремикс",
    "акустика",
    "минус",
    "кавер",
];

/// Suffixes after « - » that only describe the release, not the song.
const RELEASE_WORDS: &[&str] = &[
    "remaster",
    "remastered",
    "version",
    "edit",
    "mono",
    "stereo",
    "mix",
    "single",
    "album",
    "bonus",
    "deluxe",
    "anniversary",
    "original",
    "from",
    "recorded",
    "radio",
    "explicit",
    "clean",
];

/// Title for comparing: normalised, without bracketed parts, « - …» release suffixes and
/// `feat.` credits, punctuation dropped and `&` read as `and`.
pub fn clean_title(title: &str) -> String {
    let mut s = normalize(title);
    s = strip_brackets(&s);
    if let Some(i) = s.find(" - ") {
        s.truncate(i);
    }
    for feat in [
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " with ",
    ] {
        if let Some(i) = s.find(feat) {
            s.truncate(i);
        }
    }
    words(&s).join(" ")
}

/// The query text: the title without `feat.` credits and release suffixes, keeping what
/// names a version («Remix», «Live»), so the search brings that version first.
pub fn query_title(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut part = String::new();
    // Bracketed parts go only if they are credits or release notes.
    for c in title.chars() {
        match c {
            '(' | '[' => {
                if depth == 0 {
                    out.push_str(&part);
                    part.clear();
                }
                depth += 1;
                part.push(c);
            }
            ')' | ']' if depth > 0 => {
                depth -= 1;
                part.push(c);
                if depth == 0 {
                    if !is_release_note(&part) {
                        out.push_str(&part);
                    }
                    part.clear();
                }
            }
            _ => part.push(c),
        }
    }
    out.push_str(&part);
    let mut s = out;
    if let Some(i) = s.find(" - ")
        && is_release_note(&s[i + 3..])
    {
        s.truncate(i);
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_release_note(s: &str) -> bool {
    let w = words(&normalize(s));
    let first = w.first().map(String::as_str);
    matches!(first, Some("feat" | "ft" | "featuring" | "with"))
        || (w.iter().any(|x| RELEASE_WORDS.contains(&x.as_str()))
            && !w.iter().any(|x| VERSION_WORDS.contains(&x.as_str())))
}

fn strip_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Words of an already normalised string: letters and digits only, `&` as `and`,
/// apostrophes dropped («don't» = «dont»). Combining marks stay inside their word.
fn words(s: &str) -> Vec<String> {
    s.replace('&', " and ")
        .replace(['\'', '’', '`'], "")
        .split(|c: char| !c.is_alphanumeric() && !('\u{300}'..='\u{36f}').contains(&c))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 1.0 for the same cleaned title, else the share of shared words (Dice).
pub fn title_score(a: &str, b: &str) -> f32 {
    let (a, b) = (clean_title(a), clean_title(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    // Spaces differ between services («Bang Bang» / «BangBang»).
    if a.replace(' ', "") == b.replace(' ', "") {
        return 0.95;
    }
    let wa: HashSet<&str> = a.split(' ').collect();
    let wb: HashSet<&str> = b.split(' ').collect();
    let shared = wa.intersection(&wb).count() as f32;
    2.0 * shared / (wa.len() + wb.len()) as f32
}

fn artist_key(name: &str) -> String {
    words(&normalize(name)).join(" ")
}

fn same_artist(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    // «Simon» in «Simon and Garfunkel», «Tyler» in «Tyler, The Creator»: whole words only.
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    short.chars().count() >= 3 && format!(" {long} ").contains(&format!(" {short} "))
}

/// How well the artists agree: 1.0 when the main artist and all the others are there,
/// 0.7 for the main one alone, less when only a featured artist is found, 0 for none.
/// Featured artists also count when YouTube puts them into the title.
pub fn artist_score(source: &[String], cand: &YtTrack) -> f32 {
    if source.is_empty() {
        return 0.5;
    }
    let theirs: Vec<String> = cand.artists.iter().map(|a| artist_key(&a.name)).collect();
    let title = format!(" {} ", words(&normalize(&cand.title)).join(" "));
    let found: Vec<bool> = source
        .iter()
        .map(|a| {
            let k = artist_key(a);
            theirs.iter().any(|t| same_artist(&k, t))
                || (!k.is_empty() && title.contains(&format!(" {k} ")))
        })
        .collect();
    let others = &found[1..];
    let share = |default: f32| {
        if others.is_empty() {
            default
        } else {
            others.iter().filter(|f| **f).count() as f32 / others.len() as f32
        }
    };
    if found[0] {
        0.7 + 0.3 * share(1.0)
    } else {
        0.4 * share(0.0)
    }
}

/// 1.0 within ±3 s, then less; −0.5 past 30 s (a radio edit or a video with an intro);
/// 0.5 when either side does not know.
pub fn duration_score(source: Option<u64>, cand_ms: u64) -> f32 {
    let Some(src) = source else { return 0.5 };
    if cand_ms == 0 {
        return 0.5;
    }
    match src.abs_diff(cand_ms) {
        0..=3_000 => 1.0,
        3_001..=7_000 => 0.6,
        7_001..=15_000 => 0.2,
        15_001..=30_000 => 0.0,
        _ => -0.5,
    }
}

fn version_words(title: &str) -> HashSet<String> {
    // «Re-Recorded» (an artist's new recording of an old hit) is one word here.
    let t = normalize(title)
        .replace("re-record", "rerecord")
        .replace("re record", "rerecord");
    words(&t)
        .into_iter()
        .map(|w| {
            if w.starts_with("rerecord") {
                "rerecorded".to_owned()
            } else {
                w
            }
        })
        .filter(|w| VERSION_WORDS.contains(&w.as_str()))
        .collect()
}

/// The candidate's score, or `None` when it cannot be the same song.
pub fn score(src: &SourceTrack, cand: &YtTrack) -> Option<f32> {
    let title = title_score(&src.title, &cand.title);
    if title < 0.5 {
        return None;
    }
    let artist = artist_score(&src.artists, cand);
    let duration = duration_score(src.duration_ms, cand.duration_ms);
    // Someone else's song with the same name: only a matching length lets it through
    // (names written in another script, «Земфира» / «Zemfira»).
    if artist == 0.0 && duration < 1.0 {
        return None;
    }
    let kind = match cand.kind {
        VideoKind::Audio => 0.05,
        VideoKind::Unknown => 0.0,
        VideoKind::Video => -0.05,
    };
    let version = if version_words(&src.title) == version_words(&cand.title) {
        0.0
    } else {
        0.35
    };
    Some(0.45 * title + 0.3 * artist + 0.2 * duration + kind - version)
}

/// The best candidate at or above [`THRESHOLD`], with its score; the earlier one (YouTube's
/// order) wins a tie.
pub fn best_match<'a>(src: &SourceTrack, cands: &'a [YtTrack]) -> Option<(&'a YtTrack, f32)> {
    let mut best: Option<(&YtTrack, f32)> = None;
    for c in cands {
        if let Some(s) = score(src, c)
            && s >= THRESHOLD
            && best.is_none_or(|(_, b)| s > b)
        {
            best = Some((c, s));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_core::{ArtistRef, TrackId};

    fn src(artists: &[&str], title: &str, ms: Option<u64>) -> SourceTrack {
        SourceTrack {
            title: title.into(),
            artists: artists.iter().map(|a| (*a).to_owned()).collect(),
            duration_ms: ms,
        }
    }

    fn yt(id: &str, artists: &[&str], title: &str, ms: u64, kind: VideoKind) -> YtTrack {
        YtTrack {
            id: TrackId::from(id),
            title: title.into(),
            artists: artists
                .iter()
                .map(|a| ArtistRef {
                    id: None,
                    name: (*a).to_owned(),
                })
                .collect(),
            album: None,
            duration_ms: ms,
            thumbnail: None,
            kind,
            track_no: None,
        }
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Panama - 2015 Remaster"), "panama");
        assert_eq!(
            clean_title("Here Comes The Sun - Remastered 2009"),
            "here comes the sun"
        );
        assert_eq!(
            clean_title("expectations (feat. Chappell Roan)"),
            "expectations"
        );
        assert_eq!(
            clean_title("Get Lucky [Radio Edit] feat. Pharrell"),
            "get lucky"
        );
        assert_eq!(clean_title("Don’t Stop Me Now"), "dont stop me now");
        assert_eq!(clean_title("Rock & Roll"), "rock and roll");
        assert_eq!(clean_title("Beyoncé — Halo"), "beyonce halo");
        assert_eq!(clean_title("Искала (Remastered)"), "искала");
        assert_eq!(clean_title("Йод"), "йод");
    }

    #[test]
    fn query_keeps_versions() {
        assert_eq!(query_title("Panama - 2015 Remaster"), "Panama");
        assert_eq!(
            query_title("expectations (feat. Chappell Roan)"),
            "expectations"
        );
        assert_eq!(query_title("Midnight City (Live)"), "Midnight City (Live)");
        assert_eq!(
            query_title("Song - Acoustic Version"),
            "Song - Acoustic Version"
        );
        assert_eq!(
            query_title("Blue (Da Ba Dee) - Gabry Ponte Ice Pop Radio"),
            "Blue (Da Ba Dee)"
        );
        assert_eq!(query_title("Plain"), "Plain");
    }

    #[test]
    fn title_scores() {
        assert_eq!(
            title_score("Panama - 2015 Remaster", "Panama (2015 Remaster)"),
            1.0
        );
        assert_eq!(
            title_score("expectations (feat. Chappell Roan)", "expectations"),
            1.0
        );
        assert!(title_score("Take On Me", "Take On Me (1985 Version)") >= 1.0);
        assert!(title_score("Take On Me", "Take Me Home") < 0.7);
        assert_eq!(title_score("Love", "Hate"), 0.0);
    }

    #[test]
    fn artist_scores() {
        let c = yt(
            "a",
            &["Olivia Rodrigo", "Chappell Roan"],
            "expectations",
            0,
            VideoKind::Audio,
        );
        assert_eq!(
            artist_score(&["Olivia Rodrigo".into(), "Chappell Roan".into()], &c),
            1.0
        );
        let main_only = yt(
            "a",
            &["Olivia Rodrigo"],
            "expectations",
            0,
            VideoKind::Audio,
        );
        assert_eq!(
            artist_score(
                &["Olivia Rodrigo".into(), "Chappell Roan".into()],
                &main_only
            ),
            0.7
        );
        let feat_in_title = yt(
            "a",
            &["Daft Punk"],
            "Get Lucky (feat. Pharrell Williams)",
            0,
            VideoKind::Audio,
        );
        assert_eq!(
            artist_score(
                &["Daft Punk".into(), "Pharrell Williams".into()],
                &feat_in_title
            ),
            1.0
        );
        let duo = yt(
            "a",
            &["Simon & Garfunkel"],
            "The Boxer",
            0,
            VideoKind::Audio,
        );
        assert_eq!(
            artist_score(&["Simon".into(), "Garfunkel".into()], &duo),
            1.0
        );
        let other = yt("a", &["Someone Else"], "The Boxer", 0, VideoKind::Audio);
        assert_eq!(artist_score(&["Simon & Garfunkel".into()], &other), 0.0);
        assert_eq!(
            artist_score(
                &["ADÉLA".into()],
                &yt("a", &["Adela"], "x", 0, VideoKind::Audio)
            ),
            1.0
        );
        // Two letters are not enough to count as a part of a longer name.
        assert_eq!(
            artist_score(
                &["DJ".into()],
                &yt("a", &["DJ Snake"], "x", 0, VideoKind::Audio)
            ),
            0.0
        );
    }

    #[test]
    fn durations() {
        assert_eq!(duration_score(Some(200_000), 202_900), 1.0);
        assert_eq!(duration_score(Some(200_000), 196_000), 0.6);
        assert_eq!(duration_score(Some(200_000), 260_000), -0.5);
        assert_eq!(duration_score(None, 200_000), 0.5);
        assert_eq!(duration_score(Some(200_000), 0), 0.5);
    }

    #[test]
    fn picks_audio_over_video_and_rejects_versions() {
        let s = src(
            &["Daft Punk", "Pharrell Williams", "Nile Rodgers"],
            "Get Lucky (feat. Pharrell Williams and Nile Rodgers)",
            Some(369_000),
        );
        let cands = [
            yt(
                "live",
                &["Daft Punk"],
                "Get Lucky (Live)",
                369_500,
                VideoKind::Audio,
            ),
            yt(
                "video",
                &["Daft Punk"],
                "Get Lucky (Official Video) ft. Pharrell Williams, Nile Rodgers",
                248_000,
                VideoKind::Video,
            ),
            yt(
                "radio",
                &["Daft Punk", "Pharrell Williams", "Nile Rodgers"],
                "Get Lucky (Radio Edit)",
                248_000,
                VideoKind::Audio,
            ),
            yt(
                "album",
                &["Daft Punk", "Pharrell Williams", "Nile Rodgers"],
                "Get Lucky",
                369_000,
                VideoKind::Audio,
            ),
        ];
        let (best, s1) = best_match(&s, &cands).unwrap();
        assert_eq!(best.id.as_str(), "album");
        assert!(s1 > 0.95, "{s1}");
        assert!(
            score(&s, &cands[0]).unwrap() < THRESHOLD,
            "live is another recording"
        );
        let rerecorded = yt(
            "rr",
            &["Daft Punk"],
            "Get Lucky (Re-Recorded)",
            369_000,
            VideoKind::Audio,
        );
        assert!(score(&s, &rerecorded).unwrap() < THRESHOLD);

        // Without the album version, the radio edit (another length) still beats the video.
        let (best, _) = best_match(&s, &cands[..3]).unwrap();
        assert_eq!(best.id.as_str(), "radio");
    }

    #[test]
    fn not_found_below_threshold() {
        let s = src(&["Ashe"], "Stop The Wedding!", Some(198_840));
        let cands = [
            yt(
                "x",
                &["Someone"],
                "Stop The Wedding",
                150_000,
                VideoKind::Audio,
            ),
            yt(
                "y",
                &["Ashe"],
                "Moral of the Story",
                201_000,
                VideoKind::Audio,
            ),
        ];
        assert!(best_match(&s, &cands).is_none());
        assert!(best_match(&s, &[]).is_none());
    }

    #[test]
    fn another_script_passes_on_exact_length() {
        let s = src(&["Земфира"], "Искала", Some(215_000));
        let c = yt("z", &["Zemfira"], "Искала", 216_000, VideoKind::Audio);
        assert!(best_match(&s, std::slice::from_ref(&c)).is_some());
        let off = yt("z", &["Zemfira"], "Искала", 240_000, VideoKind::Audio);
        assert!(best_match(&s, &[off]).is_none());
    }

    #[test]
    fn pasted_line_without_duration() {
        let s = src(&["a-ha"], "Take On Me", None);
        let c = yt("t", &["a-ha"], "Take On Me", 225_000, VideoKind::Audio);
        let (_, sc) = best_match(&s, std::slice::from_ref(&c)).unwrap();
        assert!((sc - 0.9).abs() < 1e-4, "{sc}");
    }
}
