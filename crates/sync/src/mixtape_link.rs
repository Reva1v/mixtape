//! `mixtape://` links: what the app opens from a browser extension, the OS (the protocol handler
//! on the PC, intent filters on the phone) and the other device.
//!
//! Two forms, scheme and host case-insensitive, a trailing `/` and a `#fragment` ignored:
//! - `mixtape://play?ids=<id>[,<id>…][&at=<ms>]`: the form Droplet Island hands to the phone. The
//!   first id plays at `at` (missing or garbage means 0), the rest follow as the queue; repeats stay,
//!   invalid ids are dropped, and a link with no valid id is nothing.
//! - `mixtape://open?url=<percent-encoded URL>`: wraps any link [`url_import::parse_link`] reads;
//!   this is what the browser extension sends.

use crate::url_import::{self, Link};
use mixtape_core::TrackId;
use url::form_urlencoded;

/// More ids than this in one link are cut off (the PC sends up to about 300), the phone's cap too.
const MAX_IDS: usize = 500;

/// What a `mixtape://` link asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MixtapeLink {
    /// `ids[0]` plays now at `at_ms`, the rest follow it in order.
    Play { ids: Vec<TrackId>, at_ms: u64 },
    /// A YouTube Music page or track to open.
    Open(Link),
}

/// The link's request, or `None` for another scheme or host, a `play` without a valid id and an
/// `open` whose `url` is not a link this app understands.
pub fn parse(url: &str) -> Option<MixtapeLink> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("mixtape") {
        return None;
    }
    let rest = rest.split_once('#').map_or(rest, |(r, _)| r);
    let (head, query) = rest.split_once('?').unwrap_or((rest, ""));
    let head = head.trim_end_matches('/');
    // The first occurrence of a parameter wins, like on the phone; values come percent-decoded.
    let param = |key: &str| {
        form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    if head.eq_ignore_ascii_case("play") {
        let ids: Vec<TrackId> = param("ids")?
            .split(',')
            .map(str::trim)
            .filter(|s| s.len() == 11 && TrackId::is_valid(s))
            .take(MAX_IDS)
            .map(TrackId::new)
            .collect();
        let at_ms = param("at").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        (!ids.is_empty()).then_some(MixtapeLink::Play { ids, at_ms })
    } else if head.eq_ignore_ascii_case("open") {
        url_import::parse_link(&param("url")?).map(MixtapeLink::Open)
    } else {
        None
    }
}

/// `mixtape://open?url=<percent-encoded share_url(link)>`: the link that opens `link` in the app.
pub fn open_url(link: &Link) -> String {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("url", &url_import::share_url(link))
        .finish();
    format!("mixtape://open?{query}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_core::{AlbumId, ArtistId, PlaylistId};

    const A: &str = "dQw4w9WgXcQ";
    const B: &str = "9bZkp7q19f0";
    const C: &str = "kJQP7kiw5Fk";
    const PL: &str = "PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI";
    const OLAK: &str = "OLAK5uy_kvwmmSDRVnqzAJ6bxFXAiDpcBsAH9FYvA";
    const ALBUM: &str = "MPREb_4pL8gzRtw1p";
    const UC: &str = "UCuAXFkgsw1L7xaCfnd5JJOw";

    fn play(u: &str) -> (Vec<String>, u64) {
        match parse(u) {
            Some(MixtapeLink::Play { ids, at_ms }) => {
                (ids.iter().map(|t| t.as_str().to_owned()).collect(), at_ms)
            }
            other => panic!("{u}: {other:?}"),
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn encode(s: &str) -> String {
        form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }

    fn all_kinds() -> [Link; 5] {
        [
            Link::Track(TrackId::from(A)),
            Link::Album(AlbumId::from(ALBUM)),
            Link::Playlist(PlaylistId::from(PL)),
            Link::Playlist(PlaylistId::from(OLAK)),
            Link::Artist(ArtistId::from(UC)),
        ]
    }

    #[test]
    fn a_normal_play_link() {
        assert_eq!(
            play(&format!("mixtape://play?at=83500&ids={A},{B},{C}")),
            (strs(&[A, B, C]), 83_500)
        );
        assert_eq!(play(&format!("mixtape://play?ids={A}")), (strs(&[A]), 0));
    }

    #[test]
    fn at_missing_or_garbage_is_zero() {
        for at in ["", "abc", "-5", "1.5", "99999999999999999999999"] {
            assert_eq!(
                play(&format!("mixtape://play?at={at}&ids={A}")).1,
                0,
                "{at}"
            );
        }
        assert_eq!(
            play(&format!("mixtape://play?ids={A}&at=1000")),
            (strs(&[A]), 1000)
        );
        assert_eq!(
            play(&format!("mixtape://play?at=1&at=2&ids={A}&ids={B}")),
            (strs(&[A]), 1)
        );
        // Spaces around the number (a `+` is a space in a query) don't make it garbage.
        assert_eq!(
            play(&format!("mixtape://play?at=%20500+&ids={A}")),
            (strs(&[A]), 500)
        );
    }

    #[test]
    fn encoded_commas_and_bad_ids() {
        for u in [
            format!("mixtape://play?at=1&ids={A}%2C{B}"),
            format!("mixtape://play?at=1&ids={A}%2c{B}&utm=x"),
            format!("mixtape://play?at=1&ids=short,{A},,toolongtoolong,bad!id!!!!,{B},"),
        ] {
            assert_eq!(play(&u), (strs(&[A, B]), 1), "{u}");
        }
        // Spaces around an id are not part of it; an 11-character id with a bad character is dropped.
        assert_eq!(
            play(&format!("mixtape://play?ids=%20{A}+,+{B}%20,bad!id!!!!!,{C}")).0,
            strs(&[A, B, C])
        );
        // A malformed escape makes that id invalid; the next one still plays.
        assert_eq!(
            play(&format!("mixtape://play?ids={A}%zz,{B}")).0,
            strs(&[B])
        );
    }

    #[test]
    fn repeats_stay_and_the_list_is_capped() {
        assert_eq!(
            play(&format!("mixtape://play?ids={A},{B},{A},{A}")).0,
            strs(&[A, B, A, A])
        );
        let many = vec![A; MAX_IDS + 1].join(",");
        assert_eq!(play(&format!("mixtape://play?ids={many}")).0.len(), 500);
        let exact = vec![B; 500].join(",");
        assert_eq!(play(&format!("mixtape://play?ids={exact}")).0.len(), 500);
        let some = vec![B; 300].join(",");
        assert_eq!(play(&format!("mixtape://play?ids={some}")).0.len(), 300);
    }

    #[test]
    fn play_without_a_valid_id_is_nothing() {
        for u in [
            "mixtape://play",
            "mixtape://play?at=5",
            "mixtape://play?ids=",
            "mixtape://play?ids=,,,",
            "mixtape://play?ids=short,bad!id!!!!",
            "mixtape://play?at=5&ids=abc",
        ] {
            assert_eq!(parse(u), None, "{u}");
        }
    }

    #[test]
    fn open_with_each_kind() {
        for l in all_kinds() {
            let enc = encode(&url_import::share_url(&l));
            assert_eq!(
                parse(&format!("mixtape://open?url={enc}")),
                Some(MixtapeLink::Open(l.clone())),
                "{l:?}"
            );
            // A sender that forgets to encode still gets the page.
            let raw = format!("mixtape://open?url={}", url_import::share_url(&l));
            assert_eq!(parse(&raw), Some(MixtapeLink::Open(l.clone())), "{raw}");
        }
        // The page URL the extension saw, tracking parameters and all.
        let enc = encode(&format!(
            "https://music.youtube.com/watch?v={A}&si=abc&list={PL}"
        ));
        assert_eq!(
            parse(&format!("mixtape://open?url={enc}")),
            Some(MixtapeLink::Open(Link::Track(TrackId::from(A))))
        );
    }

    #[test]
    fn open_url_round_trips() {
        for l in all_kinds() {
            let u = open_url(&l);
            assert!(
                u.starts_with("mixtape://open?url=https%3A%2F%2Fmusic.youtube.com%2F"),
                "{u}"
            );
            assert_eq!(parse(&u), Some(MixtapeLink::Open(l)), "{u}");
        }
    }

    #[test]
    fn tolerates_case_slash_and_fragment() {
        let l = Link::Track(TrackId::from(A));
        let u = open_url(&l).replace("mixtape://open", "MIXTAPE://OPEN/");
        assert_eq!(parse(&format!("  {u}#frag\n")), Some(MixtapeLink::Open(l)));
        assert_eq!(
            play(&format!("MIXTAPE://PLAY/?at=1&ids={A},{B}#frag")),
            (strs(&[A, B]), 1)
        );
    }

    #[test]
    fn other_links_are_not_ours() {
        let enc = "https%3A%2F%2Fyoutu.be%2FdQw4w9WgXcQ";
        for u in [
            String::new(),
            "hello".into(),
            A.into(),
            format!("https://open?url={enc}"),
            format!("https://music.youtube.com/watch?v={A}"),
            format!("mixtape:open?url={enc}"),
            format!("mixtape://x/open?url={enc}"),
            format!("mixtape://opened?url={enc}"),
            format!("mixtape://pause?ids={A}"),
            format!("mixtape://playlist?ids={A}"),
            format!("mixtape://unknown?url={enc}"),
        ] {
            assert_eq!(parse(&u), None, "{u}");
        }
    }

    #[test]
    fn open_with_a_bad_url_is_nothing() {
        for u in [
            "mixtape://open",
            "mixtape://open?url=",
            "mixtape://open?url=garbage",
            "mixtape://open?url=https%3A%2F%2Fexample.com%2Fwatch%3Fv%3DdQw4w9WgXcQ",
            "mixtape://open?url=https%3A%2F%2Fmusic.youtube.com%2F",
            "mixtape://open?url=%C3%28",
            "mixtape://open?other=https%3A%2F%2Fyoutu.be%2FdQw4w9WgXcQ",
        ] {
            assert_eq!(parse(u), None, "{u}");
        }
    }
}
