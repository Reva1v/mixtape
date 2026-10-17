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
}
