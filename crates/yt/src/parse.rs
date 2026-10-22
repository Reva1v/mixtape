//! Navigation over InnerTube JSON. Responses are deep and change shape often, so the code
//! reads them through `serde_json::Value` and small helpers, never through rigid structs:
//! a missing field degrades one item instead of failing the whole page.

use crate::api::{VideoKind, YtAlbumCard, YtArtistCard, YtPlaylistCard, YtTrack};
use mixtape_core::{AlbumId, AlbumRef, AlbumType, ArtistId, ArtistRef, PlaylistId, TrackId};
use serde_json::Value;

/// Follows a path of object keys and array indices, e.g. `at(v, &["a", "0", "b"])`.
pub fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, key| match cur {
        Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => cur.get(*key),
    })
}

/// Every value stored under `key` anywhere inside `v`, depth-first.
pub fn find_all<'a>(v: &'a Value, key: &str) -> Vec<&'a Value> {
    fn walk<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
        match v {
            Value::Object(o) => {
                for (k, child) in o {
                    if k == key {
                        out.push(child);
                    }
                    walk(child, key, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|child| walk(child, key, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, key, &mut out);
    out
}

pub fn find_first<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(o) => o.get(key).or_else(|| o.values().find_map(|c| find_first(c, key))),
        Value::Array(a) => a.iter().find_map(|c| find_first(c, key)),
        _ => None,
    }
}

/// `{runs: [{text}]}` or `{simpleText}` → plain text.
pub fn text(v: Option<&Value>) -> String {
    let Some(v) = v else { return String::new() };
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return s.to_owned();
    }
    runs(Some(v)).iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect()
}

pub fn runs(v: Option<&Value>) -> &[Value] {
    v.and_then(|v| v.get("runs")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn run_text(run: &Value) -> &str {
    run.get("text").and_then(Value::as_str).unwrap_or("")
}

pub fn browse_id(v: &Value) -> Option<&str> {
    at(v, &["navigationEndpoint", "browseEndpoint", "browseId"])
        .or_else(|| at(v, &["browseEndpoint", "browseId"]))
        .and_then(Value::as_str)
}

pub fn page_type(v: &Value) -> Option<&str> {
    find_first(v.get("navigationEndpoint").unwrap_or(v), "pageType").and_then(Value::as_str)
}

/// The largest thumbnail URL anywhere under `v`.
pub fn thumbnail(v: &Value) -> Option<String> {
    let list = find_first(v, "thumbnails")?.as_array()?;
    list.iter()
        .max_by_key(|t| t.get("width").and_then(Value::as_u64).unwrap_or(0))
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(|u| if u.starts_with("//") { format!("https:{u}") } else { u.to_owned() })
}

/// "3:37" / "1:02:03" → milliseconds.
pub fn duration_ms(s: &str) -> Option<u64> {
    let mut total = 0u64;
    for part in s.trim().split(':') {
        total = total * 60 + part.trim().parse::<u64>().ok()?;
    }
    Some(total * 1000)
}

/// First run of digits, ignoring thin and non-breaking spaces: "1 234 трека" → 1234.
pub fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || matches!(c, ' ' | '\u{a0}' | '\u{202f}' | ','))
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

pub fn year(s: &str) -> Option<u16> {
    s.split(|c: char| !c.is_ascii_digit())
        .find(|w| w.len() == 4)
        .and_then(|w| w.parse().ok())
        .filter(|y| (1900..=2100).contains(y))
}

pub fn album_type(s: &str) -> Option<AlbumType> {
    let s = s.to_lowercase();
    if s.contains("сингл") || s.contains("single") {
        Some(AlbumType::Single)
    } else if s == "ep"
        || s.contains("мини-альбом")
        || s.contains("міні-альбом")
        || s.split_whitespace().any(|w| w == "ep")
    {
        Some(AlbumType::Ep)
    } else if s.contains("альбом") || s.contains("album") {
        Some(AlbumType::Album)
    } else {
        None
    }
}

/// Artists linked in a list of runs; unlinked names count too (`Various Artists`),
/// separators (", ", " и ", " і ", " та ", " & ", " • ") do not.
pub fn artists_from_runs(runs: &[Value]) -> Vec<ArtistRef> {
    let mut out = Vec::new();
    for run in runs {
        let name = run_text(run).trim();
        let linked = browse_id(run);
        match (linked, page_type(run)) {
            (Some(id), Some("MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL")) => {
                out.push(ArtistRef { id: Some(ArtistId::new(id)), name: name.to_owned() })
            }
            (Some(_), _) => {}
            (None, _) if is_separator(name) => {}
            (None, _) => out.push(ArtistRef { id: None, name: name.to_owned() }),
        }
    }
    out
}

fn is_separator(s: &str) -> bool {
    matches!(s, "" | "," | "&" | "•" | "и" | "і" | "та" | "and" | "x" | "×" | "feat." | "ft.")
}

fn album_from_runs(runs: &[Value]) -> Option<AlbumRef> {
    runs.iter().find_map(|r| {
        let id = browse_id(r)?;
        (page_type(r) == Some("MUSIC_PAGE_TYPE_ALBUM") || id.starts_with("MPRE"))
            .then(|| AlbumRef { id: AlbumId::new(id), title: run_text(r).to_owned() })
    })
}

pub fn video_kind(v: &Value) -> VideoKind {
    match find_first(v, "musicVideoType").and_then(Value::as_str) {
        Some("MUSIC_VIDEO_TYPE_ATV") => VideoKind::Audio,
        Some("MUSIC_VIDEO_TYPE_OMV" | "MUSIC_VIDEO_TYPE_UGC" | "MUSIC_VIDEO_TYPE_OFFICIAL_SOURCE_MUSIC") => {
            VideoKind::Video
        }
        _ => VideoKind::Unknown,
    }
}

fn flex_column(item: &Value, i: usize) -> Option<&Value> {
    at(item, &["flexColumns", &i.to_string(), "musicResponsiveListItemFlexColumnRenderer", "text"])
}

/// A song row (`musicResponsiveListItemRenderer`) from a playlist, album or likes shelf.
/// Unplayable rows (removed videos) have no `videoId` and are skipped.
pub fn list_item_track(item: &Value) -> Option<YtTrack> {
    let video_id = at(item, &["playlistItemData", "videoId"])
        .or_else(|| find_first(flex_column(item, 0)?, "videoId"))
        .and_then(Value::as_str)?;
    let title = text(flex_column(item, 0));
    let col1 = runs(flex_column(item, 1));
    let col2 = runs(flex_column(item, 2));
    let mut artists = artists_from_runs(col1);
    // Album pages put play counts in column 1 and no artists at all.
    if col1.iter().all(|r| browse_id(r).is_none()) && col1.len() == 1 && leading_number(run_text(&col1[0])).is_some()
    {
        artists.clear();
    }
    let album = album_from_runs(col2).or_else(|| album_from_runs(col1));
    let duration = find_all(item.get("fixedColumns").unwrap_or(&Value::Null), "text")
        .into_iter()
        .map(|t| text(Some(t)))
        .find_map(|s| duration_ms(&s))
        .unwrap_or(0);
    let track_no = item.get("index").map(|i| text(Some(i))).and_then(|s| s.trim().parse().ok());
    Some(YtTrack {
        id: TrackId::new(video_id),
        title,
        artists,
        album,
        duration_ms: duration,
        thumbnail: item.get("thumbnail").and_then(thumbnail),
        kind: video_kind(item),
        track_no,
    })
}

/// A card (`musicTwoRowItemRenderer`) linking to an album or single.
pub fn two_row_album(item: &Value) -> Option<YtAlbumCard> {
    let title_run = runs(item.get("title")).first()?;
    let id = browse_id(title_run).or_else(|| browse_id(item))?;
    if !id.starts_with("MPRE") {
        return None;
    }
    let sub = runs(item.get("subtitle"));
    let sub_text = text(item.get("subtitle"));
    Some(YtAlbumCard {
        id: AlbumId::new(id),
        title: run_text(title_run).to_owned(),
        artists: artists_from_runs(sub)
            .into_iter()
            .filter(|a| a.id.is_some() || (album_type(&a.name).is_none() && year(&a.name).is_none()))
            .collect(),
        year: sub.iter().find_map(|r| year(run_text(r))),
        album_type: sub.first().and_then(|r| album_type(run_text(r))).or_else(|| album_type(&sub_text)),
        thumbnail: item.get("thumbnailRenderer").and_then(thumbnail),
    })
}

/// A card linking to a playlist (`VL…`). "New playlist" and auto playlists without an id
/// are skipped by the caller through `None`.
pub fn two_row_playlist(item: &Value) -> Option<YtPlaylistCard> {
    let title_run = runs(item.get("title")).first()?;
    let id = browse_id(title_run).or_else(|| browse_id(item))?;
    if !id.starts_with("VL") {
        return None;
    }
    let sub = runs(item.get("subtitle"));
    Some(YtPlaylistCard {
        id: PlaylistId::new(id),
        title: run_text(title_run).to_owned(),
        author: sub.iter().find(|r| browse_id(r).is_some()).map(|r| run_text(r).to_owned()),
        track_count: sub.iter().rev().find_map(|r| leading_number(run_text(r))),
        thumbnail: item.get("thumbnailRenderer").and_then(thumbnail),
    })
}

/// An artist row in the library (`MPLAUC…` links to the library view of channel `UC…`).
pub fn list_item_artist(item: &Value) -> Option<YtArtistCard> {
    let id = browse_id(item)?;
    let id = id.strip_prefix("MPLA").unwrap_or(id);
    if !id.starts_with("UC") {
        return None;
    }
    Some(YtArtistCard {
        id: ArtistId::new(id),
        name: text(flex_column(item, 0)),
        thumbnail: item.get("thumbnail").and_then(thumbnail),
    })
}

pub fn two_row_artist(item: &Value) -> Option<YtArtistCard> {
    let title_run = runs(item.get("title")).first()?;
    let id = browse_id(title_run).or_else(|| browse_id(item))?;
    let kind = page_type(title_run).or_else(|| page_type(item));
    (id.starts_with("UC") && kind != Some("MUSIC_PAGE_TYPE_USER_CHANNEL")).then(|| YtArtistCard {
        id: ArtistId::new(id),
        name: run_text(title_run).to_owned(),
        thumbnail: item.get("thumbnailRenderer").and_then(thumbnail),
    })
}

/// A queue row (`playlistPanelVideoRenderer`) from `next`: radio, mixes, the watch page.
/// The byline reads "Artist, Artist • Album • 2020".
pub fn panel_track(item: &Value) -> Option<YtTrack> {
    let video_id = item.get("videoId").and_then(Value::as_str)?;
    let byline = runs(item.get("longBylineText"));
    // Artists come before the first " • ".
    let first_dot = byline.iter().position(|r| run_text(r).trim() == "•").unwrap_or(byline.len());
    Some(YtTrack {
        id: TrackId::new(video_id),
        title: text(item.get("title")),
        artists: artists_from_runs(&byline[..first_dot]),
        album: album_from_runs(byline),
        duration_ms: duration_ms(&text(item.get("lengthText"))).unwrap_or(0),
        thumbnail: item.get("thumbnail").and_then(thumbnail),
        kind: video_kind(item),
        track_no: None,
    })
}

/// Rows of the first song shelf on a page, plus the token for the next page.
pub fn shelf_items(page: &Value) -> (Vec<&Value>, Option<String>) {
    for key in ["musicPlaylistShelfRenderer", "musicShelfRenderer"] {
        if let Some(shelf) = find_first(page, key) {
            let contents = shelf.get("contents").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
            let token = continuation(contents).or_else(|| next_continuation(shelf));
            return (contents.iter().collect(), token);
        }
    }
    (Vec::new(), None)
}

/// Rows from a `browse {continuation}` response, plus the token for the page after it.
pub fn continuation_items(resp: &Value) -> (Vec<&Value>, Option<String>) {
    if let Some(items) = find_first(resp, "continuationItems").and_then(Value::as_array) {
        return (items.iter().collect(), continuation(items));
    }
    if let Some(c) = resp.get("continuationContents").and_then(Value::as_object).and_then(|o| o.values().next()) {
        let items = c.get("contents").or_else(|| c.get("items")).and_then(Value::as_array);
        let items = items.map(Vec::as_slice).unwrap_or(&[]);
        return (items.iter().collect(), continuation(items).or_else(|| next_continuation(c)));
    }
    (Vec::new(), None)
}

fn continuation(items: &[Value]) -> Option<String> {
    items.iter().rev().find_map(|i| {
        let r = i.get("continuationItemRenderer")?;
        find_first(r, "token").and_then(Value::as_str).map(str::to_owned)
    })
}

fn next_continuation(shelf: &Value) -> Option<String> {
    at(shelf, &["continuations", "0", "nextContinuationData", "continuation"])
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Grid items (`musicTwoRowItemRenderer`) of a library tab or "see all" page.
pub fn grid_items(page: &Value) -> (Vec<&Value>, Option<String>) {
    let Some(grid) = find_first(page, "gridRenderer") else { return (Vec::new(), None) };
    let items = grid.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    (items.iter().collect(), continuation(items).or_else(|| next_continuation(grid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artist_run(name: &str, id: &str) -> Value {
        json!({"text": name, "navigationEndpoint": {"browseEndpoint": {"browseId": id,
            "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}})
    }

    fn col(runs: Vec<Value>) -> Value {
        json!({"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": runs}}})
    }

    #[test]
    fn playlist_row() {
        let row = json!({
            "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [
                {"url": "https://x/a=w60-h60", "width": 60}, {"url": "https://x/a=w120-h120", "width": 120}]}}},
            "flexColumns": [
                col(vec![json!({"text": "Song", "navigationEndpoint": {"watchEndpoint": {"videoId": "abcdefghijk",
                    "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": "MUSIC_VIDEO_TYPE_ATV"}}}}})]),
                col(vec![artist_run("A", "UCa"), json!({"text": " и "}), artist_run("B", "UCb")]),
                col(vec![json!({"text": "Album", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_x"}}})]),
            ],
            "fixedColumns": [{"musicResponsiveListItemFixedColumnRenderer": {"text": {"runs": [{"text": "3:37"}]}}}],
            "playlistItemData": {"videoId": "abcdefghijk"}
        });
        let t = list_item_track(&row).unwrap();
        assert_eq!(t.id.as_str(), "abcdefghijk");
        assert_eq!(t.title, "Song");
        assert_eq!(t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(t.album.unwrap().id.as_str(), "MPREb_x");
        assert_eq!(t.duration_ms, 217_000);
        assert_eq!(t.thumbnail.as_deref(), Some("https://x/a=w120-h120"));
        assert_eq!(t.kind, VideoKind::Audio);
    }

    #[test]
    fn album_row_has_plays_instead_of_artists() {
        let row = json!({
            "flexColumns": [
                col(vec![json!({"text": "Intro", "navigationEndpoint": {"watchEndpoint": {"videoId": "abcdefghijk"}}})]),
                col(vec![]),
                col(vec![json!({"text": "10\u{a0}млн прослушиваний"})]),
            ],
            "index": {"runs": [{"text": "3"}]},
            "playlistItemData": {"videoId": "abcdefghijk"}
        });
        let t = list_item_track(&row).unwrap();
        assert!(t.artists.is_empty());
        assert_eq!(t.track_no, Some(3));
    }

    #[test]
    fn removed_video_is_skipped() {
        let row = json!({"flexColumns": [col(vec![json!({"text": "Видео удалено"})])]});
        assert!(list_item_track(&row).is_none());
    }

    #[test]
    fn album_card() {
        let card = json!({
            "title": {"runs": [{"text": "Lo-fi", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_1"}}}]},
            "subtitle": {"runs": [{"text": "Сингл"}, {"text": " • "}, artist_run("Lee", "UCl"), {"text": " • "}, {"text": "2023"}]}
        });
        let a = two_row_album(&card).unwrap();
        assert_eq!(a.album_type, Some(AlbumType::Single));
        assert_eq!(a.year, Some(2023));
        assert_eq!(a.artists.len(), 1);
        assert_eq!(a.artists[0].name, "Lee");
    }

    #[test]
    fn playlist_card() {
        let card = json!({
            "title": {"runs": [{"text": "Like", "navigationEndpoint": {"browseEndpoint": {"browseId": "VLPL1"}}}]},
            "subtitle": {"runs": [{"text": "Me", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCme"}}}, {"text": " • "}, {"text": "1\u{a0}277 треков"}]}
        });
        let p = two_row_playlist(&card).unwrap();
        assert_eq!(p.id.as_str(), "VLPL1");
        assert_eq!(p.track_count, Some(1277));
        assert_eq!(p.author.as_deref(), Some("Me"));
        let new_playlist = json!({"title": {"runs": [{"text": "Новый", "navigationEndpoint": {"createPlaylistEndpoint": {}}}]}});
        assert!(two_row_playlist(&new_playlist).is_none());
    }

    #[test]
    fn continuation_tokens() {
        let page = json!({"x": {"musicPlaylistShelfRenderer": {"contents": [
            {"musicResponsiveListItemRenderer": {}},
            {"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "T1"}}}}
        ]}}});
        let (items, token) = shelf_items(&page);
        assert_eq!(items.len(), 2);
        assert_eq!(token.as_deref(), Some("T1"));
        let next = json!({"onResponseReceivedActions": [{"appendContinuationItemsAction": {"continuationItems": [
            {"musicResponsiveListItemRenderer": {}}]}}]});
        let (items, token) = continuation_items(&next);
        assert_eq!(items.len(), 1);
        assert!(token.is_none());
    }

    #[test]
    fn numbers() {
        assert_eq!(duration_ms("1:02:03"), Some(3_723_000));
        assert_eq!(leading_number("23 трека"), Some(23));
        assert_eq!(year("Альбом • 2020"), Some(2020));
        assert_eq!(album_type("EP"), Some(AlbumType::Ep));
    }

    #[test]
    fn ukrainian_labels() {
        assert_eq!(album_type("Сингл"), Some(AlbumType::Single));
        assert_eq!(album_type("Сингли та міні-альбоми"), Some(AlbumType::Single));
        assert_eq!(album_type("Міні-альбом"), Some(AlbumType::Ep));
        assert_eq!(album_type("Альбом"), Some(AlbumType::Album));
        assert_eq!(album_type("Альбоми"), Some(AlbumType::Album));
        assert_eq!(leading_number("1\u{a0}277 треків"), Some(1277));
        let runs = [
            artist_run("A", "UCa"),
            json!({"text": " і "}),
            json!({"text": "B"}),
            json!({"text": " та "}),
            json!({"text": "C"}),
        ];
        let names: Vec<String> = artists_from_runs(&runs).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["A", "B", "C"]);
    }
}
