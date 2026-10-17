//! Importing a Spotify playlist or album, or a pasted «Artist — Title» list, as a local
//! playlist of YouTube Music songs.
//!
//! - [`parse_link`] recognises `open.spotify.com/{playlist,album}/<id>` links (also with
//!   `intl-xx/`, `embed/`, a query string or no scheme) and `spotify:playlist:<id>` URIs.
//! - [`page`] reads the public embed page (no login, no API key). The embed lists at most
//!   [`page::EMBED_LIMIT`] tracks; longer playlists continue through the web player's public
//!   query endpoint with the anonymous token the embed page carries. If that fails the list
//!   is imported cut, and the summary says so.
//! - [`parse_lines`] reads a pasted track list, one «Artist — Title» per line.
//! - [`matching`] scores YouTube Music search results against a source track and
//!   [`import`] runs the searches and stores the result.

pub mod import;
pub mod matching;
pub mod page;

use mixtape_core::tr;
use serde::Serialize;
use ts_rs::TS;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotifyKind {
    Playlist,
    Album,
}

impl SpotifyKind {
    fn path(self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::Album => "album",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotifyLink {
    pub kind: SpotifyKind,
    /// Base62 id, 22 characters.
    pub id: String,
}

/// A track to look for on YouTube Music: from Spotify or from a pasted line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceTrack {
    pub title: String,
    /// Main artist first; may be empty for a pasted line without an artist.
    pub artists: Vec<String>,
    #[ts(optional, type = "number")]
    pub duration_ms: Option<u64>,
}

impl SourceTrack {
    /// «Artist, Artist — Title», as the not-found list is copied.
    pub fn line(&self) -> String {
        if self.artists.is_empty() {
            self.title.clone()
        } else {
            format!("{} — {}", self.artists.join(", "), self.title)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpotifyError {
    #[error("{}: {}", tr("No connection to Spotify", "Немає з'єднання зі Spotify", "Нет соединения со Spotify"), .0)]
    Network(String),
    #[error(
        "{}",
        tr(
            "Spotify didn't open this link: the playlist may be private or deleted",
            "Spotify не відкрив посилання: плейлист може бути приватним або видаленим",
            "Spotify не открыл ссылку: плейлист может быть приватным или удалённым",
        )
    )]
    NotFound,
    #[error("{}: {}", tr("Couldn't read the Spotify page", "Не вдалося прочитати сторінку Spotify", "Не удалось прочитать страницу Spotify"), .0)]
    Parse(String),
}

impl From<reqwest::Error> for SpotifyError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<SpotifyError> for mixtape_core::AppError {
    fn from(e: SpotifyError) -> Self {
        let kind = match e {
            SpotifyError::Network(_) => mixtape_core::ErrorKind::Network,
            SpotifyError::NotFound => mixtape_core::ErrorKind::NotFound,
            SpotifyError::Parse(_) => mixtape_core::ErrorKind::Invalid,
        };
        Self::new(kind, e.to_string())
    }
}

/// Recognises a Spotify playlist or album link (see the module docs).
pub fn parse_link(s: &str) -> Option<SpotifyLink> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("spotify:") {
        let (kind, id) = rest.rsplit_once(':')?;
        // `spotify:user:<name>:playlist:<id>` is the old form of the same thing.
        let kind = kind.rsplit(':').next()?;
        return link(kind, id);
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
    if !matches!(
        url.host_str()?.to_ascii_lowercase().as_str(),
        "open.spotify.com" | "play.spotify.com"
    ) {
        return None;
    }
    let mut segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    if segs.first().is_some_and(|s| s.starts_with("intl-")) {
        segs.remove(0);
    }
    if segs.first() == Some(&"embed") {
        segs.remove(0);
    }
    // `/user/<name>/playlist/<id>` links from old clients.
    if segs.first() == Some(&"user") && segs.len() >= 4 {
        segs.drain(..2);
    }
    match segs.as_slice() {
        [kind, id] => link(kind, id),
        _ => None,
    }
}

fn link(kind: &str, id: &str) -> Option<SpotifyLink> {
    let kind = match kind {
        "playlist" => SpotifyKind::Playlist,
        "album" => SpotifyKind::Album,
        _ => return None,
    };
    let ok = id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| SpotifyLink {
        kind,
        id: id.to_owned(),
    })
}

/// Separators between artist and title, with spaces around them so `a-ha` and
/// `Jay-Z` stay whole.
const SEPARATORS: &[&str] = &[" — ", " – ", " - ", " -- "];

/// Reads a pasted track list: one «Artist — Title» per line (`-`, `–` or `—`). Leading
/// numbering («1.», «01)») and a trailing duration («3:45», «(3:45)») are understood;
/// empty lines are skipped. A line without a separator is a title alone.
pub fn parse_lines(text: &str) -> Vec<SourceTrack> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<SourceTrack> {
    let mut s = strip_numbering(line.trim());
    let duration_ms = strip_duration(&mut s);
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let split = SEPARATORS
        .iter()
        .filter_map(|sep| s.find(sep).map(|i| (i, sep.len())))
        .min_by_key(|(i, _)| *i)
        // `Artist—Title` without spaces: only the long dashes, a hyphen is part of names.
        .or_else(|| {
            s.find(['—', '–'])
                .map(|i| (i, s[i..].chars().next().map_or(1, char::len_utf8)))
        });
    let (artist, title) = match split {
        Some((i, len)) => (s[..i].trim(), s[i + len..].trim()),
        None => ("", s),
    };
    if title.is_empty() {
        return None;
    }
    Some(SourceTrack {
        title: title.to_owned(),
        artists: split_artists(artist),
        duration_ms,
    })
}

fn strip_numbering(s: &str) -> &str {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 4 {
        return s;
    }
    let rest = &s[digits..];
    match rest.strip_prefix(['.', ')']) {
        Some(r) if r.starts_with(' ') => r.trim_start(),
        _ => s,
    }
}

/// Removes a trailing `m:ss` / `(m:ss)` / `[h:mm:ss]` and returns it in ms.
fn strip_duration(s: &mut &str) -> Option<u64> {
    let t = s.trim_end();
    let (body, inner) = match t.chars().last()? {
        ')' | ']' => {
            let open = t.rfind(['(', '['])?;
            (&t[..open], &t[open + 1..t.len() - 1])
        }
        _ => {
            let start = t.rfind(char::is_whitespace).map_or(0, |i| i + 1);
            (&t[..start], &t[start..])
        }
    };
    let parts: Vec<&str> = inner.trim().split(':').collect();
    if !(2..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let n: Vec<u64> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    if n[1..].iter().any(|&x| x >= 60) {
        return None;
    }
    let secs = n.iter().fold(0, |acc, x| acc * 60 + x);
    // Strip the separator that may stand before the duration («Title - 3:45»).
    *s = body
        .trim_end()
        .trim_end_matches(['-', '–', '—', '|'])
        .trim_end();
    Some(secs * 1000)
}

/// Splits «A, B & C feat. D» into names. A comma inside a name (`Tyler, The Creator`) is
/// split too; matching still finds the first part in the YouTube artist.
pub fn split_artists(s: &str) -> Vec<String> {
    let mut s = s.to_owned();
    for sep in [
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " & ",
        " x ",
        " × ",
        "; ",
        " / ",
    ] {
        s = s.replace(sep, ",");
    }
    s.split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "37i9dQZF1DXcBWIGoYBM5M";

    fn pl() -> Option<SpotifyLink> {
        Some(SpotifyLink {
            kind: SpotifyKind::Playlist,
            id: ID.into(),
        })
    }

    #[test]
    fn links() {
        let album = Some(SpotifyLink {
            kind: SpotifyKind::Album,
            id: "4m2880jivSbbyEGAKfITCa".into(),
        });
        let cases: Vec<(String, Option<SpotifyLink>)> = vec![
            (format!("https://open.spotify.com/playlist/{ID}"), pl()),
            (
                format!("https://open.spotify.com/playlist/{ID}?si=abc123&pi=x"),
                pl(),
            ),
            (
                format!("https://open.spotify.com/intl-de/playlist/{ID}"),
                pl(),
            ),
            (
                format!("https://open.spotify.com/intl-pt-BR/playlist/{ID}?si=1"),
                pl(),
            ),
            (
                format!("https://open.spotify.com/embed/playlist/{ID}"),
                pl(),
            ),
            (format!("open.spotify.com/playlist/{ID}"), pl()),
            (format!("  http://OPEN.spotify.com/playlist/{ID}/  "), pl()),
            (
                format!("https://open.spotify.com/user/spotify/playlist/{ID}"),
                pl(),
            ),
            (format!("spotify:playlist:{ID}"), pl()),
            (format!("spotify:user:spotify:playlist:{ID}"), pl()),
            (
                "https://open.spotify.com/album/4m2880jivSbbyEGAKfITCa".into(),
                album.clone(),
            ),
            (
                "https://open.spotify.com/intl-uk/album/4m2880jivSbbyEGAKfITCa?si=q".into(),
                album,
            ),
            // Rejected.
            (
                "https://open.spotify.com/track/3XmwAZpIMNpTJy8faw3ajD".into(),
                None,
            ),
            (
                "https://open.spotify.com/artist/4tZwfgrHOc3mvqYlEYSvVi".into(),
                None,
            ),
            ("https://open.spotify.com/playlist/short".into(), None),
            (format!("https://example.com/playlist/{ID}"), None),
            (
                format!("https://open.spotify.com/playlist/{ID}/extra"),
                None,
            ),
            (format!("ftp://open.spotify.com/playlist/{ID}"), None),
            ("https://music.youtube.com/playlist?list=PLx".into(), None),
            (String::new(), None),
            (ID.into(), None),
        ];
        for (input, want) in cases {
            assert_eq!(parse_link(&input), want, "input: {input:?}");
        }
    }

    fn t(artists: &[&str], title: &str, dur: Option<u64>) -> SourceTrack {
        SourceTrack {
            title: title.into(),
            artists: artists.iter().map(|s| (*s).to_owned()).collect(),
            duration_ms: dur,
        }
    }

    #[test]
    fn lines() {
        let text = "a-ha — Take On Me\n\
                    \n\
                    Daft Punk - Get Lucky (feat. Pharrell Williams) - Radio Edit\n\
                    1. Queen – Bohemian Rhapsody 5:55\n\
                    02) Земфира — Искала (3:35)\n\
                    Jay-Z & Kanye West — Otis\n\
                    Just A Title\n\
                    Simon & Garfunkel—The Boxer [5:08]\n\
                    Artist — \n";
        assert_eq!(
            parse_lines(text),
            vec![
                t(&["a-ha"], "Take On Me", None),
                t(
                    &["Daft Punk"],
                    "Get Lucky (feat. Pharrell Williams) - Radio Edit",
                    None
                ),
                t(&["Queen"], "Bohemian Rhapsody", Some(355_000)),
                t(&["Земфира"], "Искала", Some(215_000)),
                t(&["Jay-Z", "Kanye West"], "Otis", None),
                t(&[], "Just A Title", None),
                t(&["Simon", "Garfunkel"], "The Boxer", Some(308_000)),
            ]
        );
    }

    #[test]
    fn durations_need_valid_clock() {
        assert_eq!(parse_lines("A — Song 99"), vec![t(&["A"], "Song 99", None)]);
        assert_eq!(parse_lines("A — 7:61"), vec![t(&["A"], "7:61", None)]);
        assert_eq!(
            parse_lines("A — Song - 1:02:03"),
            vec![t(&["A"], "Song", Some(3_723_000))]
        );
    }

    #[test]
    fn splits_artists() {
        assert_eq!(
            split_artists("Olivia Rodrigo,\u{a0}Chappell Roan"),
            ["Olivia Rodrigo", "Chappell Roan"]
        );
        assert_eq!(split_artists("A feat. B; C"), ["A", "B", "C"]);
        assert!(split_artists("  ").is_empty());
    }

    #[test]
    fn line_for_copy() {
        assert_eq!(t(&["A", "B"], "Song", None).line(), "A, B — Song");
        assert_eq!(t(&[], "Song", None).line(), "Song");
    }
}
