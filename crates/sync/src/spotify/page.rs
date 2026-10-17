//! Reading a playlist or album from Spotify's public pages, without a login.
//!
//! `open.spotify.com/embed/{playlist,album}/<id>` is server-rendered: its `__NEXT_DATA__`
//! script holds the name and the first [`EMBED_LIMIT`] tracks (title, artists as one
//! comma-separated line, duration), plus an anonymous web-player token. Checked on
//! 2026-10-17: «Today's Top Hits» (50) came whole, «All Out 80s» (150) only its first 100.
//!
//! For the rest the web player's `pathfinder` query `fetchPlaylist` is asked page by page
//! with that token; it also tells the real track count. It is not a documented API (the
//! query hash may change), so any failure there just leaves the list cut and the summary
//! says how much was read. The public Web API refuses this token (`429 QUOTA_EXCEEDED`).

use super::{SourceTrack, SpotifyError, SpotifyKind, SpotifyLink};
use serde_json::{Value, json};

/// Tracks the embed page lists at most.
pub const EMBED_LIMIT: usize = 100;
/// Pages asked from `pathfinder` at most (100 tracks each).
const MAX_PAGES: usize = 100;
const PAGE: usize = 100;
const PATHFINDER: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
/// `fetchPlaylist` persisted query, as the web player sent it (checked 2026-10-17).
const FETCH_PLAYLIST_HASH: &str =
    "19ff1327c29e99c208c86d7a9d8f1929cfdf3d3202a0ff4253c821f1901aa94d";

/// What Spotify told about a playlist or album.
#[derive(Debug, Clone, PartialEq)]
pub struct SpotifyList {
    pub kind: SpotifyKind,
    pub name: String,
    pub tracks: Vec<SourceTrack>,
    /// How many tracks the list has, when Spotify said so.
    pub total: Option<u32>,
    /// Anonymous web-player token from the embed page, for the rest of a long playlist.
    pub token: Option<String>,
}

impl SpotifyList {
    /// The list is longer than what was read, or may be (the embed cap was hit and the
    /// real count is unknown).
    pub fn is_cut(&self) -> bool {
        match self.total {
            Some(total) => (self.tracks.len() as u32) < total,
            None => self.tracks.len() >= EMBED_LIMIT,
        }
    }
}

/// The `__NEXT_DATA__` JSON of an embed page.
fn next_data(html: &str) -> Option<Value> {
    let start = html.find("id=\"__NEXT_DATA__\"")?;
    let body = &html[start..];
    let open = body.find('>')? + 1;
    let close = body[open..].find("</script>")?;
    serde_json::from_str(&body[open..open + close]).ok()
}

/// Parses an embed page. `total` stays unknown: the embed does not tell it.
pub fn parse_embed(html: &str) -> Result<SpotifyList, SpotifyError> {
    let data = next_data(html).ok_or_else(|| SpotifyError::Parse("__NEXT_DATA__".into()))?;
    let state = data
        .pointer("/props/pageProps/state")
        .ok_or_else(|| SpotifyError::Parse("state".into()))?;
    let entity = state
        .pointer("/data/entity")
        .ok_or(SpotifyError::NotFound)?;
    let kind = match entity.get("type").and_then(Value::as_str) {
        Some("playlist") => SpotifyKind::Playlist,
        Some("album") => SpotifyKind::Album,
        other => return Err(SpotifyError::Parse(format!("type {other:?}"))),
    };
    let name = entity
        .get("name")
        .or_else(|| entity.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let tracks = entity
        .get("trackList")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|t| {
            t.get("entityType")
                .and_then(Value::as_str)
                .is_none_or(|k| k == "track")
        })
        .filter_map(|t| {
            let title = t.get("title")?.as_str()?.trim();
            (!title.is_empty()).then(|| SourceTrack {
                title: title.to_owned(),
                artists: split_list(
                    t.get("subtitle")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
                duration_ms: t.get("duration").and_then(Value::as_u64).filter(|d| *d > 0),
            })
        })
        .collect();
    let token = state
        .pointer("/settings/session/accessToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    Ok(SpotifyList {
        kind,
        name,
        tracks,
        total: None,
        token,
    })
}

/// The embed's artist line is «A,\u{a0}B»: split on commas only (names keep their `&`).
fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .collect()
}

/// One `fetchPlaylist` page.
#[derive(Debug, Clone, PartialEq)]
pub struct PathfinderPage {
    /// Songs only: episodes and local files are skipped.
    pub tracks: Vec<SourceTrack>,
    /// Items on the page, skipped ones included (they take places in the offsets).
    pub items: usize,
    /// Items in the whole playlist.
    pub total: Option<u32>,
}

pub fn parse_pathfinder(resp: &Value) -> Result<PathfinderPage, SpotifyError> {
    let content = resp
        .pointer("/data/playlistV2/content")
        .ok_or_else(|| SpotifyError::Parse("playlistV2.content".into()))?;
    let total = content
        .get("totalCount")
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    let items = content
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let tracks = items
        .iter()
        .filter_map(|i| {
            let t = i.pointer("/itemV2/data")?;
            if t.get("__typename").and_then(Value::as_str) != Some("Track") {
                return None;
            }
            let title = t.get("name")?.as_str()?.trim();
            let artists = t
                .pointer("/artists/items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| a.pointer("/profile/name").and_then(Value::as_str))
                .map(|a| a.trim().to_owned())
                .filter(|a| !a.is_empty())
                .collect();
            let duration_ms = t
                .pointer("/trackDuration/totalMilliseconds")
                .and_then(Value::as_u64)
                .filter(|d| *d > 0);
            (!title.is_empty()).then(|| SourceTrack {
                title: title.to_owned(),
                artists,
                duration_ms,
            })
        })
        .collect();
    Ok(PathfinderPage {
        tracks,
        items: items.len(),
        total,
    })
}

fn pathfinder_body(id: &str, offset: usize) -> Value {
    json!({
        "variables": { "uri": format!("spotify:playlist:{id}"), "offset": offset, "limit": PAGE, "enableWatchFeedEntrypoint": false },
        "operationName": "fetchPlaylist",
        "extensions": { "persistedQuery": { "version": 1, "sha256Hash": FETCH_PLAYLIST_HASH } },
    })
}

async fn get_text(req: reqwest::RequestBuilder) -> Result<String, SpotifyError> {
    let resp = req.send().await?;
    match resp.status() {
        s if s.is_success() => Ok(resp.text().await?),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN => {
            Err(SpotifyError::NotFound)
        }
        s => Err(SpotifyError::Network(format!("HTTP {}", s.as_u16()))),
    }
}

/// Reads a playlist or album: the embed page, then the rest of a long playlist.
pub async fn fetch(
    http: &reqwest::Client,
    link: &SpotifyLink,
) -> Result<SpotifyList, SpotifyError> {
    let url = format!(
        "https://open.spotify.com/embed/{}/{}",
        link.kind.path(),
        link.id
    );
    let html = get_text(http.get(&url).header("Accept-Language", "en")).await?;
    let mut list = parse_embed(&html)?;
    if list.kind == SpotifyKind::Playlist
        && list.tracks.len() >= EMBED_LIMIT
        && let Some(token) = list.token.clone()
    {
        continue_playlist(http, &link.id, &token, &mut list).await;
    }
    tracing::info!(kind = ?list.kind, read = list.tracks.len(), total = ?list.total, "spotify list read");
    Ok(list)
}

/// Appends the tracks after the embed's first page; stops quietly on any failure. Places
/// are counted by items, so skipped episodes do not shift the offsets.
async fn continue_playlist(http: &reqwest::Client, id: &str, token: &str, list: &mut SpotifyList) {
    let mut offset = list.tracks.len();
    let mut skipped = 0u32;
    for _ in 0..MAX_PAGES {
        let req = http
            .post(PATHFINDER)
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(pathfinder_body(id, offset).to_string());
        let page = get_text(req)
            .await
            .and_then(|t| {
                serde_json::from_str::<Value>(&t).map_err(|e| SpotifyError::Parse(e.to_string()))
            })
            .and_then(|v| parse_pathfinder(&v));
        let page = match page {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, offset, "spotify: the rest of the playlist could not be read");
                break;
            }
        };
        skipped += (page.items - page.tracks.len()) as u32;
        offset += page.items;
        // The total counts episodes too; the cut check compares songs with songs.
        list.total = page.total.map(|t| t.saturating_sub(skipped)).or(list.total);
        list.tracks.extend(page.tracks);
        if page.items == 0 || page.total.is_some_and(|t| offset as u32 >= t) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn embed_playlist() {
        let l = parse_embed(&fixture("spotify_embed_playlist.html")).unwrap();
        assert_eq!(l.kind, SpotifyKind::Playlist);
        assert_eq!(l.name, "Today’s Top Hits");
        assert_eq!(l.token.as_deref(), Some("test-token"));
        assert_eq!(l.total, None);
        assert_eq!(l.tracks.len(), 6);
        assert_eq!(
            l.tracks[0],
            SourceTrack {
                title: "expectations (feat. Chappell Roan)".into(),
                artists: vec!["Olivia Rodrigo".into(), "Chappell Roan".into()],
                duration_ms: Some(223_935),
            }
        );
        assert_eq!(l.tracks[5].artists, ["KAROL G", "Judeline", "rusowsky"]);
        assert!(!l.is_cut());
    }

    #[test]
    fn embed_album() {
        let l = parse_embed(&fixture("spotify_embed_album.html")).unwrap();
        assert_eq!(l.kind, SpotifyKind::Album);
        assert_eq!(l.name, "Random Access Memories");
        let titles: Vec<&str> = l.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Give Life Back to Music",
                "The Game of Love",
                "Giorgio by Moroder"
            ]
        );
        assert_eq!(l.tracks[2].duration_ms, Some(544_626));
    }

    #[test]
    fn embed_errors() {
        assert!(matches!(
            parse_embed("<html>nothing</html>"),
            Err(SpotifyError::Parse(_))
        ));
        let no_entity = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{}}}}}</script>"#;
        assert!(matches!(
            parse_embed(no_entity),
            Err(SpotifyError::NotFound)
        ));
    }

    #[test]
    fn pathfinder_page() {
        let v: Value = serde_json::from_str(&fixture("spotify_pathfinder_page.json")).unwrap();
        let p = parse_pathfinder(&v).unwrap();
        assert_eq!(p.total, Some(150));
        assert_eq!(p.items, 4);
        assert_eq!(p.tracks.len(), 3, "the episode is skipped");
        assert!(
            p.tracks
                .iter()
                .all(|t| !t.artists.is_empty() && t.duration_ms.is_some())
        );
        assert!(parse_pathfinder(&json!({"errors": []})).is_err());
    }

    #[test]
    fn cut_detection() {
        let t = SourceTrack {
            title: "x".into(),
            artists: vec![],
            duration_ms: None,
        };
        let mut l = SpotifyList {
            kind: SpotifyKind::Playlist,
            name: String::new(),
            tracks: vec![t; EMBED_LIMIT],
            total: None,
            token: None,
        };
        assert!(
            l.is_cut(),
            "the embed cap was hit, the real size is unknown"
        );
        l.total = Some(150);
        assert!(l.is_cut());
        l.total = Some(100);
        assert!(!l.is_cut());
    }
}
