//! Parsing YouTube Music / YouTube links pasted into "Add by link".
//!
//! Accepted hosts: `music.youtube.com`, `youtube.com` (`www.`, `m.`) and `youtu.be`;
//! the scheme may be omitted. Bare ids work too. Rules:
//! - `watch?v=X` (with or without `&list=…`), `youtu.be/X`, `/shorts/X`, `/embed/X`,
//!   `/live/X`, a bare 11-character id → [`Link::Track`]. A watch link always means the
//!   track, even when it carries a playlist.
//! - `playlist?list=Y`, `/browse/VLY`, a bare `PL…`/`OLAK5uy_…`/`RDCLAK…` id →
//!   [`Link::Playlist`]. Playlist ids are returned **without** the `VL` browse prefix,
//!   exactly as they appear in `list=` (`PL…`, `OLAK5uy_…`, `RDCLAK…`); add `VL` when a
//!   browse id is needed. `OLAK5uy_…` lists are album playlists (see
//!   [`Link::is_album_playlist`]). Auto-generated mixes (`RD…` other than `RDCLAK…`)
//!   and the special `LL`/`LM` lists are rejected.
//! - `/browse/MPREb_…` or a bare `MPREb_…` → [`Link::Album`].
//! - `/channel/UC…`, `/browse/UC…` or a bare `UC…` → [`Link::Artist`]. `@handle` links
//!   need a network lookup and are rejected.
//!
//! SoundCloud and Bandcamp links ([`parse_external`]) are imported through yt-dlp without
//! an account (`crate::external`):
//! - `soundcloud.com/<user>/<track>` (a private `/s-…` token is kept) → [`ExternalKind::Track`],
//!   `/<user>/sets/<set>` → [`ExternalKind::Set`], `/<user>` and `/<user>/tracks` →
//!   [`ExternalKind::Profile`] (the user's uploads). Other profile tabs, site pages and
//!   `on.soundcloud.com` short links (need a redirect) are rejected.
//! - `<artist>.bandcamp.com/track/<slug>` → [`ExternalKind::Track`], `/album/<slug>` →
//!   [`ExternalKind::Album`]. Custom domains are not recognised.

use mixtape_core::{AlbumId, ArtistId, PlaylistId, TrackId};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Track(TrackId),
    /// `MPREb_…` browse id.
    Album(AlbumId),
    /// List id without the `VL` prefix.
    Playlist(PlaylistId),
    /// `UC…` channel id.
    Artist(ArtistId),
}

impl Link {
    /// An `OLAK5uy_…` playlist, which YouTube Music uses for albums.
    pub fn is_album_playlist(&self) -> bool {
        matches!(self, Self::Playlist(p) if p.as_str().starts_with("OLAK5uy_"))
    }
}

const HOSTS: &[&str] = &[
    "music.youtube.com",
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "youtu.be",
    "www.youtu.be",
];

/// Recognizes a track, album, playlist or artist link (see the module docs).
pub fn parse_link(s: &str) -> Option<Link> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(link) = parse_bare(s) {
        return Some(link);
    }
    let with_scheme = if s.contains("://") {
        s.to_owned()
    } else {
        format!("https://{s}")
    };
    let url = Url::parse(&with_scheme).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    if !HOSTS.contains(&host.as_str()) {
        return None;
    }
    let query = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().to_owned())
    };
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();

    if host.ends_with("youtu.be") {
        return segs.first().and_then(|id| track(id));
    }
    match segs.as_slice() {
        ["watch"] => query("v").as_deref().and_then(track),
        ["shorts" | "embed" | "live" | "v", id, ..] => track(id),
        ["playlist"] => query("list").as_deref().and_then(playlist),
        ["browse", id] => {
            if let Some(rest) = id.strip_prefix("VL") {
                playlist(rest)
            } else if id.starts_with("MPREb_") {
                album(id)
            } else {
                artist(id)
            }
        }
        ["channel", id, ..] => artist(id),
        _ => None,
    }
}

fn parse_bare(s: &str) -> Option<Link> {
    if s.contains(['/', '.', '?', '=']) {
        return None;
    }
    if s.starts_with("MPREb_") {
        return album(s);
    }
    if s.starts_with("UC") && s.len() == 24 {
        return artist(s);
    }
    if s.len() == 11 {
        return track(s);
    }
    playlist(s.strip_prefix("VL").unwrap_or(s))
}

fn id_chars(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn track(id: &str) -> Option<Link> {
    (id.len() == 11 && id_chars(id)).then(|| Link::Track(TrackId::from(id)))
}

fn album(id: &str) -> Option<Link> {
    (id.starts_with("MPREb_") && id.len() > 6 && id_chars(id))
        .then(|| Link::Album(AlbumId::from(id)))
}

fn artist(id: &str) -> Option<Link> {
    (id.starts_with("UC") && id.len() == 24 && id_chars(id))
        .then(|| Link::Artist(ArtistId::from(id)))
}

fn playlist(id: &str) -> Option<Link> {
    let known = ["PL", "OLAK5uy_", "RDCLAK", "UU", "FL"]
        .iter()
        .any(|p| id.starts_with(p));
    (known && id.len() >= 12 && id_chars(id)).then(|| Link::Playlist(PlaylistId::from(id)))
}

/// Where an [`ExternalLink`] points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    SoundCloud,
    Bandcamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalKind {
    Track,
    /// A Bandcamp album.
    Album,
    /// A SoundCloud set (playlist, EP or album).
    Set,
    /// A SoundCloud user's uploads.
    Profile,
}

/// A SoundCloud or Bandcamp link; `url` is the canonical `https://` form without the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalLink {
    pub site: Site,
    pub kind: ExternalKind,
    pub url: String,
}

/// First path segments that are SoundCloud pages, not users.
const SC_RESERVED: &[&str] = &[
    "discover", "search", "stream", "feed", "you", "upload", "charts", "settings", "messages",
    "notifications", "pages", "terms-of-use", "mobile", "people", "jobs", "popular", "signin",
    "signup", "logout", "imprint", "connect", "home", "tags", "tracks", "playlists", "sets",
    "artists", "creators", "premium", "go", "pro", "help", "community-guidelines",
];

/// Profile tabs that are not a single track.
const SC_TABS: &[&str] = &[
    "albums", "popular-tracks", "likes", "reposts", "followers", "following", "comments",
    "spotlight", "sets", "tracks",
];

/// Recognizes a SoundCloud or Bandcamp link (see the module docs).
pub fn parse_external(s: &str) -> Option<ExternalLink> {
    let s = s.trim();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return None;
    }
    let with_scheme = if s.contains("://") { s.to_owned() } else { format!("https://{s}") };
    let url = Url::parse(&with_scheme).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    if !segs.iter().all(|s| slug(s)) {
        return None;
    }
    if matches!(host.as_str(), "soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com") {
        return soundcloud(&segs);
    }
    let artist = host.strip_suffix(".bandcamp.com")?;
    if artist.is_empty() || artist.contains('.') || matches!(artist, "www" | "daily" | "m") {
        return None;
    }
    let kind = match segs.as_slice() {
        ["track", _] => ExternalKind::Track,
        ["album", _] => ExternalKind::Album,
        _ => return None,
    };
    Some(ExternalLink { site: Site::Bandcamp, kind, url: format!("https://{host}/{}", segs.join("/")) })
}

fn soundcloud(segs: &[&str]) -> Option<ExternalLink> {
    let user = *segs.first()?;
    if SC_RESERVED.contains(&user.to_ascii_lowercase().as_str()) {
        return None;
    }
    let token = |t: &str| t.starts_with("s-") && t.len() > 2;
    let (kind, path) = match segs {
        [_] | [_, "tracks"] => (ExternalKind::Profile, format!("{user}/tracks")),
        [_, "sets", set] => (ExternalKind::Set, format!("{user}/sets/{set}")),
        [_, "sets", set, t] if token(t) => (ExternalKind::Set, format!("{user}/sets/{set}/{t}")),
        [_, track] if !SC_TABS.contains(track) => (ExternalKind::Track, format!("{user}/{track}")),
        [_, track, t] if !SC_TABS.contains(track) && token(t) => {
            (ExternalKind::Track, format!("{user}/{track}/{t}"))
        }
        _ => return None,
    };
    Some(ExternalLink { site: Site::SoundCloud, kind, url: format!("https://soundcloud.com/{path}") })
}

fn slug(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "dQw4w9WgXcQ";
    const PL: &str = "PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI";
    const OLAK: &str = "OLAK5uy_kvwmmSDRVnqzAJ6bxFXAiDpcBsAH9FYvA";
    const RDCLAK: &str = "RDCLAK5uy_kmPRjHDECIcuVwnKsx2Ng7fyNgFKWNJFs";
    const ALBUM: &str = "MPREb_4pL8gzRtw1p";
    const UC: &str = "UCuAXFkgsw1L7xaCfnd5JJOw";

    fn t(s: &str) -> Option<Link> {
        Some(Link::Track(TrackId::from(s)))
    }
    fn p(s: &str) -> Option<Link> {
        Some(Link::Playlist(PlaylistId::from(s)))
    }

    #[test]
    fn table() {
        let a = Some(Link::Album(AlbumId::from(ALBUM)));
        let ar = Some(Link::Artist(ArtistId::from(UC)));
        let cases: Vec<(String, Option<Link>)> = vec![
            // Tracks.
            (format!("https://music.youtube.com/watch?v={V}"), t(V)),
            (
                format!("https://music.youtube.com/watch?v={V}&si=abc123"),
                t(V),
            ),
            (
                format!("https://music.youtube.com/watch?v={V}&list={OLAK}"),
                t(V),
            ),
            (
                format!("https://www.youtube.com/watch?v={V}&list={PL}&index=3"),
                t(V),
            ),
            (
                format!("https://www.youtube.com/watch?feature=share&v={V}"),
                t(V),
            ),
            (format!("https://m.youtube.com/watch?v={V}"), t(V)),
            (format!("https://youtube.com/watch?v={V}&t=42s"), t(V)),
            (format!("http://youtube.com/watch?v={V}"), t(V)),
            (format!("https://youtu.be/{V}"), t(V)),
            (format!("https://youtu.be/{V}?si=xyz&t=10"), t(V)),
            (format!("https://www.youtube.com/shorts/{V}"), t(V)),
            (format!("https://www.youtube.com/embed/{V}?start=5"), t(V)),
            (format!("https://www.youtube.com/live/{V}"), t(V)),
            (format!("music.youtube.com/watch?v={V}"), t(V)),
            (format!("youtu.be/{V}"), t(V)),
            (format!("  https://MUSIC.youtube.com/watch?v={V}  "), t(V)),
            (V.to_owned(), t(V)),
            // Playlists.
            (
                format!("https://music.youtube.com/playlist?list={PL}"),
                p(PL),
            ),
            (
                format!("https://music.youtube.com/playlist?list={PL}&si=x"),
                p(PL),
            ),
            (format!("https://www.youtube.com/playlist?list={PL}"), p(PL)),
            (
                format!("https://music.youtube.com/playlist?list={OLAK}"),
                p(OLAK),
            ),
            (
                format!("https://music.youtube.com/playlist?list={RDCLAK}"),
                p(RDCLAK),
            ),
            (format!("https://music.youtube.com/browse/VL{PL}"), p(PL)),
            (PL.to_owned(), p(PL)),
            (format!("VL{PL}"), p(PL)),
            (OLAK.to_owned(), p(OLAK)),
            // Albums.
            (
                format!("https://music.youtube.com/browse/{ALBUM}"),
                a.clone(),
            ),
            (format!("music.youtube.com/browse/{ALBUM}?si=1"), a.clone()),
            (ALBUM.to_owned(), a),
            // Artists.
            (
                format!("https://music.youtube.com/channel/{UC}"),
                ar.clone(),
            ),
            (
                format!("https://www.youtube.com/channel/{UC}/videos"),
                ar.clone(),
            ),
            (format!("https://music.youtube.com/browse/{UC}"), ar.clone()),
            (UC.to_owned(), ar),
            // Rejected.
            (String::new(), None),
            ("   ".into(), None),
            ("hello world".into(), None),
            ("https://example.com/watch?v=dQw4w9WgXcQ".into(), None),
            ("https://notyoutube.com/watch?v=dQw4w9WgXcQ".into(), None),
            ("ftp://youtube.com/watch?v=dQw4w9WgXcQ".into(), None),
            ("https://music.youtube.com/watch?v=short".into(), None),
            ("https://music.youtube.com/watch".into(), None),
            ("https://music.youtube.com/".into(), None),
            ("https://music.youtube.com/playlist?list=LM".into(), None),
            ("https://www.youtube.com/playlist?list=LL".into(), None),
            (format!("https://www.youtube.com/playlist?list=RD{V}"), None),
            ("https://www.youtube.com/@rickastley".into(), None),
            ("https://music.youtube.com/browse/FEmusic_home".into(), None),
            ("https://youtu.be/".into(), None),
            ("https://music.youtube.com/watch?v=dQw4w9WgXc!".into(), None),
        ];
        for (input, want) in cases {
            assert_eq!(parse_link(&input), want, "input: {input:?}");
        }
    }

    #[test]
    fn album_playlist_flag() {
        assert!(parse_link(OLAK).unwrap().is_album_playlist());
        assert!(!parse_link(PL).unwrap().is_album_playlist());
        assert!(!parse_link(V).unwrap().is_album_playlist());
    }

    #[test]
    fn external_table() {
        use ExternalKind::*;
        let sc = |kind, url: &str| Some(ExternalLink { site: Site::SoundCloud, kind, url: url.into() });
        let bc = |kind, url: &str| Some(ExternalLink { site: Site::Bandcamp, kind, url: url.into() });
        let cases: Vec<(&str, Option<ExternalLink>)> = vec![
            // SoundCloud tracks.
            ("https://soundcloud.com/forss/flickermood", sc(Track, "https://soundcloud.com/forss/flickermood")),
            (
                "https://soundcloud.com/forss/flickermood?si=abc&utm_source=clipboard",
                sc(Track, "https://soundcloud.com/forss/flickermood"),
            ),
            ("soundcloud.com/forss/flickermood", sc(Track, "https://soundcloud.com/forss/flickermood")),
            ("https://m.soundcloud.com/forss/flickermood/", sc(Track, "https://soundcloud.com/forss/flickermood")),
            ("https://www.soundcloud.com/forss/flickermood#t=1:00", sc(Track, "https://soundcloud.com/forss/flickermood")),
            (
                "https://soundcloud.com/a-user/secret-track/s-AbC123",
                sc(Track, "https://soundcloud.com/a-user/secret-track/s-AbC123"),
            ),
            // Sets.
            (
                "https://soundcloud.com/the-concept-band/sets/the-royal-concept-ep",
                sc(Set, "https://soundcloud.com/the-concept-band/sets/the-royal-concept-ep"),
            ),
            ("https://soundcloud.com/u/sets/s1/s-tok3n?in=x", sc(Set, "https://soundcloud.com/u/sets/s1/s-tok3n")),
            // Profiles.
            ("https://soundcloud.com/forss", sc(Profile, "https://soundcloud.com/forss/tracks")),
            ("https://soundcloud.com/forss/tracks", sc(Profile, "https://soundcloud.com/forss/tracks")),
            // Bandcamp.
            (
                "https://benprunty.bandcamp.com/track/lanius-battle",
                bc(Track, "https://benprunty.bandcamp.com/track/lanius-battle"),
            ),
            (
                "benprunty.bandcamp.com/track/lanius-battle?from=embed",
                bc(Track, "https://benprunty.bandcamp.com/track/lanius-battle"),
            ),
            (
                "https://Blazo.Bandcamp.com/album/jazz-format-mixtape-vol-1",
                bc(Album, "https://blazo.bandcamp.com/album/jazz-format-mixtape-vol-1"),
            ),
            (
                "http://blazo.bandcamp.com/album/jazz-format-mixtape-vol-1/",
                bc(Album, "https://blazo.bandcamp.com/album/jazz-format-mixtape-vol-1"),
            ),
            // Rejected.
            ("", None),
            ("https://soundcloud.com/", None),
            ("https://soundcloud.com/discover", None),
            ("https://soundcloud.com/search?q=forss", None),
            ("https://soundcloud.com/forss/likes", None),
            ("https://soundcloud.com/forss/sets", None),
            ("https://soundcloud.com/forss/albums", None),
            ("https://soundcloud.com/forss/flickermood/comments", None),
            ("https://on.soundcloud.com/AbCdEf", None),
            ("https://api.soundcloud.com/tracks/293", None),
            ("https://notsoundcloud.com/forss/flickermood", None),
            ("ftp://soundcloud.com/forss/flickermood", None),
            ("https://bandcamp.com/track/x", None),
            ("https://daily.bandcamp.com/album/x", None),
            ("https://blazo.bandcamp.com/", None),
            ("https://blazo.bandcamp.com/music", None),
            ("https://blazo.bandcamp.com/album", None),
            ("https://blazo.bandcamp.com/merch/shirt", None),
            ("https://music.blazo.com/album/x", None),
            ("https://x.y.bandcamp.com/album/x", None),
            ("https://music.youtube.com/watch?v=dQw4w9WgXcQ", None),
            ("soundcloud forss", None),
        ];
        for (input, want) in cases {
            assert_eq!(parse_external(input), want, "input: {input:?}");
        }
        // Neither parser claims the other's links.
        assert_eq!(parse_link("https://soundcloud.com/forss/flickermood"), None);
        assert_eq!(parse_link("https://blazo.bandcamp.com/album/jazz-format-mixtape-vol-1"), None);
    }
}
