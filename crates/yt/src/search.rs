//! Catalog search: the `search` endpoint (all results or one filtered kind), its continuation
//! pages and the text suggestions under the search box. Parsing follows `parse.rs`: `Value`
//! helpers, a broken item is skipped instead of failing the page.
//!
//! Items are classified by their endpoints (video id, browse page type), never by the shelf
//! or label titles, which come in the UI language (`hl=ru`). Music videos, podcasts, episodes
//! and user profiles are skipped.

use crate::api::{VideoKind, YtAlbumCard, YtArtistCard, YtPlaylistCard, YtTrack};
use crate::innertube::InnerTube;
use crate::parse::{self, at, find_all, find_first, runs, text};
use crate::{Result, YtError};
use mixtape_core::{AlbumId, AlbumRef, ArtistId, ArtistRef, PlaylistId, TrackId};
use serde_json::{Value, json};
use std::collections::HashSet;

/// What to search for: everything (with the top result card) or one filter chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchKind {
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl SearchKind {
    /// `params` of the web client's filter chips (Треки, Альбомы, Исполнители, Пользовательские
    /// плейлисты), as music.youtube.com sent them on 2026-10-17.
    pub fn params(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Tracks => Some("EgWKAQIIAWoSEAUQAxAJEAoQBBAQEBUQDhAR"),
            Self::Albums => Some("EgWKAQIYAWoSEAUQAxAJEAoQBBAQEBUQDhAR"),
            Self::Artists => Some("EgWKAQIgAWoSEAUQAxAJEAoQBBAQEBUQDhAR"),
            Self::Playlists => Some("EgeKAQQoAEABahIQBRADEAkQChAEEBAQFRAOEBE="),
        }
    }
}

/// One search result of any kind.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchItem {
    Track(YtTrack),
    Album(YtAlbumCard),
    Artist(YtArtistCard),
    Playlist(YtPlaylistCard),
}

impl SearchItem {
    fn id(&self) -> &str {
        match self {
            Self::Track(t) => t.id.as_str(),
            Self::Album(a) => a.id.as_str(),
            Self::Artist(a) => a.id.as_str(),
            Self::Playlist(p) => p.id.as_str(),
        }
    }
}

/// One page of results. `top` is the "Лучший результат" card of an all-kinds search; filtered
/// searches fill one list and usually carry a `continuation` for `search_more`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchPage {
    pub top: Option<SearchItem>,
    pub tracks: Vec<YtTrack>,
    pub albums: Vec<YtAlbumCard>,
    pub artists: Vec<YtArtistCard>,
    pub playlists: Vec<YtPlaylistCard>,
    pub continuation: Option<String>,
}

impl SearchPage {
    fn push(&mut self, item: SearchItem) {
        match item {
            SearchItem::Track(t) => self.tracks.push(t),
            SearchItem::Album(a) => self.albums.push(a),
            SearchItem::Artist(a) => self.artists.push(a),
            SearchItem::Playlist(p) => self.playlists.push(p),
        }
    }

    /// Keeps only `kind` (a filtered page has nothing else to show), drops repeats and the
    /// items that duplicate the top card.
    fn finish(mut self, kind: SearchKind) -> Self {
        let top = self.top.as_ref().map(|t| t.id().to_owned());
        let mut seen: HashSet<String> = top.into_iter().collect();
        let mut fresh = |id: &str| seen.insert(id.to_owned());
        self.tracks.retain(|t| fresh(t.id.as_str()));
        self.albums.retain(|a| fresh(a.id.as_str()));
        self.artists.retain(|a| fresh(a.id.as_str()));
        self.playlists.retain(|p| fresh(p.id.as_str()));
        if kind != SearchKind::All {
            self.top = None;
        }
        let keep = |k: SearchKind| kind == SearchKind::All || kind == k;
        if !keep(SearchKind::Tracks) {
            self.tracks.clear();
        }
        if !keep(SearchKind::Albums) {
            self.albums.clear();
        }
        if !keep(SearchKind::Artists) {
            self.artists.clear();
        }
        if !keep(SearchKind::Playlists) {
            self.playlists.clear();
        }
        self
    }
}

/// `search` body for the first page.
pub fn search_body(q: &str, kind: SearchKind) -> Value {
    let mut body = json!({ "query": q });
    if let Some(params) = kind.params() {
        body["params"] = json!(params);
    }
    body
}

/// `search` body for the page after `continuation`.
pub fn more_body(continuation: &str) -> Value {
    json!({ "continuation": continuation })
}

/// `music/get_search_suggestions` body.
pub fn suggest_body(q: &str) -> Value {
    json!({ "input": q })
}

/// A first `search` response → page, `None` if it is not a search page at all.
pub fn parse_search(resp: &Value, kind: SearchKind) -> Option<SearchPage> {
    let tabs = find_first(resp, "tabbedSearchResultsRenderer")?
        .get("tabs")?
        .as_array()?;
    // The first tab is YouTube Music, the second one is the user's library (empty until opened).
    let tab = tabs
        .iter()
        .find(|t| at(t, &["tabRenderer", "selected"]).and_then(Value::as_bool) == Some(true))
        .or_else(|| tabs.first())?;
    let mut page = SearchPage::default();
    if let Some(list) = find_first(tab, "sectionListRenderer") {
        for section in array(list.get("contents")) {
            read_section(section, &mut page);
        }
    }
    Some(page.finish(kind))
}

/// A `search {continuation}` response → page; without `continuationContents` the list is over.
pub fn parse_more(resp: &Value, kind: SearchKind) -> SearchPage {
    let mut page = SearchPage::default();
    let shelf = resp
        .get("continuationContents")
        .and_then(Value::as_object)
        .and_then(|o| o.values().next());
    if let Some(shelf) = shelf {
        read_shelf(shelf, &mut page);
    }
    page.finish(kind)
}

/// Text suggestions (and the user's matching history, first) in the order YouTube sent them.
/// Entity suggestions (songs, artists) are left out.
pub fn parse_suggestions(resp: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    find_all(resp, "searchSuggestionsSectionRenderer")
        .into_iter()
        .flat_map(|s| array(s.get("contents")))
        .filter_map(|i| {
            let r = i
                .get("searchSuggestionRenderer")
                .or_else(|| i.get("historySuggestionRenderer"))?;
            let q = at(r, &["navigationEndpoint", "searchEndpoint", "query"])
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| text(r.get("suggestion")));
            let q = q.trim().to_owned();
            (!q.is_empty()).then_some(q)
        })
        .filter(|q| seen.insert(q.clone()))
        .collect()
}

fn array(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn read_section(section: &Value, page: &mut SearchPage) {
    if let Some(card) = section.get("musicCardShelfRenderer") {
        let top = card_item(card);
        let fill = match &top {
            Some(SearchItem::Artist(a)) => Some(ArtistRef {
                id: Some(a.id.clone()),
                name: a.name.clone(),
            }),
            _ => None,
        };
        for row in array(card.get("contents")) {
            if let Some(item) = row_item(row, true, fill.as_ref()) {
                page.push(item);
            }
        }
        page.top = page.top.take().or(top);
    } else if let Some(shelf) = section.get("musicShelfRenderer") {
        read_shelf(shelf, page);
    } else if let Some(items) = section.get("itemSectionRenderer") {
        // The current layout: one row per section, its type label first ("Композиция • …").
        for row in array(items.get("contents")) {
            if let Some(shelf) = row.get("musicShelfRenderer") {
                read_shelf(shelf, page);
            } else if let Some(item) = row_item(row, true, None) {
                page.push(item);
            }
        }
    }
}

fn read_shelf(shelf: &Value, page: &mut SearchPage) {
    let rows = array(shelf.get("contents"));
    let typed = labelled(rows);
    for item in rows.iter().filter_map(|r| row_item(r, typed, None)) {
        page.push(item);
    }
    if page.continuation.is_none() {
        page.continuation = shelf_continuation(shelf, rows);
    }
}

fn shelf_continuation(shelf: &Value, rows: &[Value]) -> Option<String> {
    array(shelf.get("continuations"))
        .iter()
        .find_map(|c| {
            c.get("nextContinuationData")
                .or_else(|| c.get("reloadContinuationData"))?
                .get("continuation")?
                .as_str()
        })
        .or_else(|| {
            rows.iter().rev().find_map(|r| {
                find_first(r.get("continuationItemRenderer")?, "token").and_then(Value::as_str)
            })
        })
        .map(str::to_owned)
}

/// The subtitle of a shelf's rows starts with a type label ("Исполнитель • 73 млн…") when
/// every row starts with the same unlinked word followed by " • ". Rows of a filtered list
/// start with their artist or author instead, which differ from row to row.
fn labelled(rows: &[Value]) -> bool {
    let firsts: Vec<Option<&str>> = rows
        .iter()
        .filter_map(|r| r.get("musicResponsiveListItemRenderer"))
        .map(|r| label(runs(flex_column(r, 1))))
        .collect();
    firsts.len() >= 2 && firsts[0].is_some() && firsts.iter().all(|f| *f == firsts[0])
}

fn label(sub: &[Value]) -> Option<&str> {
    let first = sub.first()?;
    let is_dot = |r: &Value| run_text(r).trim() == "•";
    (first.get("navigationEndpoint").is_none() && sub.get(1).is_some_and(is_dot))
        .then(|| run_text(first))
}

fn flex_column(item: &Value, i: usize) -> Option<&Value> {
    at(
        item,
        &[
            "flexColumns",
            &i.to_string(),
            "musicResponsiveListItemFlexColumnRenderer",
            "text",
        ],
    )
}

fn run_text(run: &Value) -> &str {
    run.get("text").and_then(Value::as_str).unwrap_or("")
}

/// Runs split on the " • " separators.
fn segments(sub: &[Value]) -> Vec<&[Value]> {
    sub.split(|r| run_text(r).trim() == "•")
        .filter(|s| !s.is_empty())
        .collect()
}

fn is_duration(s: &str) -> bool {
    s.contains(':') && parse::duration_ms(s).is_some()
}

/// What a row or card shows, before it is classified.
struct Row<'a> {
    title: String,
    /// The endpoint the item opens: browse (album, artist, playlist) or watch (a song).
    endpoint: Option<&'a Value>,
    video_id: Option<&'a str>,
    /// Where the video type is: the title run's watch endpoint, else the whole item.
    video_source: &'a Value,
    sub: &'a [Value],
    /// The subtitle starts with a type label.
    typed: bool,
    thumbnail: Option<String>,
    /// Links of the "…" menu: (page type, browse id); rows of an all-kinds search name their
    /// artists without links and keep the ids only here.
    menu: Vec<(&'a str, &'a str)>,
    duration_ms: u64,
    /// The artist of the top card: rows under it often name nobody and link the artist only
    /// from their menu.
    card_artist: Option<&'a ArtistRef>,
}

fn menu_links(item: &Value) -> Vec<(&str, &str)> {
    let items = at(item, &["menu", "menuRenderer", "items"]);
    array(items)
        .iter()
        .filter_map(|i| {
            let b = at(
                i,
                &[
                    "menuNavigationItemRenderer",
                    "navigationEndpoint",
                    "browseEndpoint",
                ],
            )?;
            let kind = find_first(b, "pageType")?.as_str()?;
            Some((kind, b.get("browseId")?.as_str()?))
        })
        .collect()
}

/// Episodes link their show and their own page from the title instead of a watch endpoint;
/// the video type that says "episode" sits in the play button overlay, if at all.
fn podcast(v: &Value) -> bool {
    find_all(v, "pageType").into_iter().any(|t| {
        matches!(
            t.as_str(),
            Some(
                "MUSIC_PAGE_TYPE_PODCAST_SHOW_DETAIL_PAGE"
                    | "MUSIC_PAGE_TYPE_NON_MUSIC_AUDIO_TRACK_PAGE"
            )
        )
    })
}

/// A `musicResponsiveListItemRenderer` wrapper → result; `typed` says whether its subtitle
/// starts with a type label.
fn row_item<'a>(
    row: &'a Value,
    typed: bool,
    card_artist: Option<&'a ArtistRef>,
) -> Option<SearchItem> {
    let item = row.get("musicResponsiveListItemRenderer")?;
    if podcast(item) {
        return None;
    }
    let title_run = runs(flex_column(item, 0)).first();
    let watch = title_run.and_then(|r| at(r, &["navigationEndpoint", "watchEndpoint"]));
    let video_id = at(item, &["playlistItemData", "videoId"])
        .or_else(|| watch?.get("videoId"))
        .and_then(Value::as_str);
    let duration_ms = find_all(item.get("fixedColumns").unwrap_or(&Value::Null), "text")
        .into_iter()
        .find_map(|t| parse::duration_ms(&text(Some(t))))
        .unwrap_or(0);
    classify(Row {
        title: text(flex_column(item, 0)),
        endpoint: item.get("navigationEndpoint"),
        video_id,
        video_source: watch.unwrap_or(item),
        sub: runs(flex_column(item, 1)),
        typed,
        thumbnail: item.get("thumbnail").and_then(parse::thumbnail),
        menu: menu_links(item),
        duration_ms,
        card_artist,
    })
}

/// The top result card (`musicCardShelfRenderer`); its subtitle always starts with the type.
fn card_item(card: &Value) -> Option<SearchItem> {
    if podcast(card.get("title")?) || podcast(card.get("subtitle").unwrap_or(&Value::Null)) {
        return None;
    }
    let title_run = runs(card.get("title")).first();
    let endpoint = title_run
        .and_then(|r| r.get("navigationEndpoint"))
        .or_else(|| card.get("onTap"));
    let watch = endpoint.and_then(|e| e.get("watchEndpoint"));
    classify(Row {
        title: text(card.get("title")),
        endpoint,
        video_id: watch.and_then(|w| w.get("videoId")).and_then(Value::as_str),
        video_source: watch.unwrap_or(card),
        sub: runs(card.get("subtitle")),
        typed: true,
        thumbnail: card.get("thumbnail").and_then(parse::thumbnail),
        menu: menu_links(card),
        duration_ms: 0,
        card_artist: None,
    })
}

fn classify(row: Row<'_>) -> Option<SearchItem> {
    if let Some(id) = row.video_id {
        return track(&row, id).map(SearchItem::Track);
    }
    let endpoint = row.endpoint?;
    let id = parse::browse_id(endpoint)?;
    match find_first(endpoint, "pageType").and_then(Value::as_str)? {
        "MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK" => {
            album(&row, id).map(SearchItem::Album)
        }
        "MUSIC_PAGE_TYPE_ARTIST" if id.starts_with("UC") => {
            Some(SearchItem::Artist(YtArtistCard {
                id: ArtistId::new(id),
                name: row.title.clone(),
                thumbnail: row.thumbnail.clone(),
            }))
        }
        "MUSIC_PAGE_TYPE_PLAYLIST" => playlist(&row, id).map(SearchItem::Playlist),
        // Profiles (USER_CHANNEL), podcasts and anything new.
        _ => None,
    }
}

/// Artists of `segment`, without durations and years; the first one takes the id from the menu
/// when the row names it without a link. A row that names nobody under an artist card is that
/// artist's when its menu links there.
fn artists_of(row: &Row<'_>, segment: &[Value]) -> Vec<ArtistRef> {
    let mut artists: Vec<ArtistRef> = parse::artists_from_runs(segment)
        .into_iter()
        .filter(|a| a.id.is_some() || (!is_duration(&a.name) && parse::year(&a.name).is_none()))
        .collect();
    let menu_artist = row
        .menu
        .iter()
        .find(|(kind, _)| *kind == "MUSIC_PAGE_TYPE_ARTIST")
        .map(|(_, id)| ArtistId::new(*id));
    match (artists.first_mut(), menu_artist) {
        (Some(first), Some(id)) if first.id.is_none() => first.id = Some(id),
        (None, Some(id)) => artists.extend(
            row.card_artist
                .filter(|c| c.id.as_ref() == Some(&id))
                .cloned(),
        ),
        _ => {}
    }
    artists
}

fn track(row: &Row<'_>, id: &str) -> Option<YtTrack> {
    let kind = match find_first(row.video_source, "musicVideoType").and_then(Value::as_str) {
        Some("MUSIC_VIDEO_TYPE_ATV") => VideoKind::Audio,
        None => VideoKind::Unknown,
        // Music videos, podcast episodes and whatever comes next are not tracks.
        Some(_) => return None,
    };
    let segs = segments(row.sub);
    let artist_seg = segs.get(usize::from(row.typed)).copied().unwrap_or(&[]);
    let album = row.sub.iter().find_map(|r| {
        let id = parse::browse_id(r)?;
        (parse::page_type(r) == Some("MUSIC_PAGE_TYPE_ALBUM") || id.starts_with("MPRE")).then(
            || AlbumRef {
                id: AlbumId::new(id),
                title: run_text(r).to_owned(),
            },
        )
    });
    let duration_ms = row
        .sub
        .iter()
        .map(run_text)
        .find(|s| is_duration(s))
        .and_then(parse::duration_ms)
        .unwrap_or(row.duration_ms);
    Some(YtTrack {
        id: TrackId::new(id),
        title: row.title.clone(),
        artists: artists_of(row, artist_seg),
        album,
        duration_ms,
        thumbnail: row.thumbnail.clone(),
        kind,
        track_no: None,
    })
}

fn album(row: &Row<'_>, id: &str) -> Option<YtAlbumCard> {
    if !id.starts_with("MPRE") {
        return None;
    }
    let segs = segments(row.sub);
    let seg_text = |s: &[Value]| s.iter().map(run_text).collect::<String>();
    // Album rows start with their type ("Альбом", "Сингл", "EP") in every layout.
    let album_type = segs.first().and_then(|s| parse::album_type(&seg_text(s)));
    let skip_first = row.typed || album_type.is_some();
    let rest: Vec<Value> = segs
        .iter()
        .skip(usize::from(skip_first))
        .flat_map(|s| s.iter().cloned())
        .collect();
    Some(YtAlbumCard {
        id: AlbumId::new(id),
        title: row.title.clone(),
        artists: artists_of(row, &rest),
        year: row.sub.iter().find_map(|r| parse::year(run_text(r))),
        album_type,
        thumbnail: row.thumbnail.clone(),
    })
}

fn playlist(row: &Row<'_>, id: &str) -> Option<YtPlaylistCard> {
    let id = if id.starts_with("VL") {
        id.to_owned()
    } else {
        format!("VL{id}")
    };
    let segs = segments(row.sub);
    let author_seg = segs.get(usize::from(row.typed)).copied().unwrap_or(&[]);
    let author = row
        .sub
        .iter()
        .find(|r| parse::browse_id(r).is_some())
        .map(|r| run_text(r).to_owned())
        .or_else(|| Some(author_seg.iter().map(run_text).collect::<String>()))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    // Community playlists show views, editorial ones a track count: only the latter counts.
    let track_count = segs
        .iter()
        .map(|s| s.iter().map(run_text).collect::<String>())
        .find(|s| {
            let s = s.to_lowercase();
            s.contains("трек") || s.contains("track") || s.contains("песн") || s.contains("song")
        })
        .and_then(|s| parse::leading_number(&s));
    Some(YtPlaylistCard {
        id: PlaylistId::new(id),
        title: row.title.clone(),
        author,
        track_count,
        thumbnail: row.thumbnail.clone(),
    })
}

impl InnerTube {
    /// `search` {query, params?}: all kinds with the top card, or one filter chip's list
    /// (20 rows + continuation).
    pub async fn search(&self, q: &str, kind: SearchKind) -> Result<SearchPage> {
        if q.trim().is_empty() {
            return Ok(SearchPage::default());
        }
        let resp = self.call("search", search_body(q.trim(), kind)).await?;
        parse_search(&resp, kind)
            .ok_or_else(|| YtError::Parse("search: tabbedSearchResultsRenderer".into()))
    }

    /// `search` {continuation}: the next page of a filtered list; empty at the end.
    pub async fn search_more(&self, kind: SearchKind, continuation: &str) -> Result<SearchPage> {
        let resp = self.call("search", more_body(continuation)).await?;
        Ok(parse_more(&resp, kind))
    }

    /// `music/get_search_suggestions` {input}: text completions for the search box.
    pub async fn suggest(&self, q: &str) -> Result<Vec<String>> {
        if q.trim().is_empty() {
            return Ok(Vec::new());
        }
        let resp = self
            .call("music/get_search_suggestions", suggest_body(q))
            .await?;
        Ok(parse_suggestions(&resp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_core::AlbumType;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Type labels of the all-kinds layout: none of them may leak in as an artist or author.
    const LABELS: &[&str] = &[
        "Композиция",
        "Альбом",
        "Сингл",
        "EP",
        "Плейлист",
        "Видео",
        "Исполнитель",
    ];

    fn no_labels(artists: &[ArtistRef]) {
        for a in artists {
            assert!(
                !LABELS.contains(&a.name.as_str()),
                "label as artist: {artists:?}"
            );
            assert!(!is_duration(&a.name) && !a.name.is_empty(), "{artists:?}");
        }
    }

    /// Video ids of rows that are not songs (clips, episodes) in a raw response.
    fn non_song_ids(raw: &Value) -> Vec<String> {
        find_all(raw, "musicResponsiveListItemRenderer")
            .into_iter()
            .filter(|r| {
                find_first(r, "musicVideoType")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t != "MUSIC_VIDEO_TYPE_ATV")
            })
            .filter_map(|r| {
                at(r, &["playlistItemData", "videoId"])
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect()
    }

    #[test]
    fn all_kinds_with_artist_card() {
        let raw = fixture("search_all.json");
        let page = parse_search(&raw, SearchKind::All).unwrap();
        let Some(SearchItem::Artist(top)) = &page.top else {
            panic!("{:?}", page.top)
        };
        assert_eq!(
            (top.id.as_str(), top.name.as_str()),
            ("UC9TynkEiIsDxKjtDZKmiUvw", "Madeon")
        );
        assert!(top.thumbnail.is_some());
        assert!(page.continuation.is_none());
        assert!(page.tracks.len() >= 6, "{} tracks", page.tracks.len());
        assert!(!page.albums.is_empty() && !page.artists.is_empty() && !page.playlists.is_empty());
        // The card's songs name nobody: the artist comes from the card through the menu link.
        let first = &page.tracks[0];
        assert_eq!(first.id.as_str(), "pMTH_XGNozE");
        assert_eq!(first.title, "Dream Dream Dream");
        assert_eq!(first.artists.len(), 1);
        assert_eq!(
            first.artists[0].id.as_ref().map(|a| a.as_str()),
            Some("UC9TynkEiIsDxKjtDZKmiUvw")
        );
        assert_eq!(first.artists[0].name, "Madeon");
        assert_eq!(first.duration_ms, 235_000);
        assert_eq!(first.kind, VideoKind::Audio);
        for t in &page.tracks {
            no_labels(&t.artists);
            assert!(!t.artists.is_empty(), "{t:?}");
            assert_eq!(t.kind, VideoKind::Audio);
        }
        for a in &page.albums {
            no_labels(&a.artists);
            assert!(
                a.id.as_str().starts_with("MPRE") && a.album_type.is_some() && a.year.is_some(),
                "{a:?}"
            );
        }
        for p in &page.playlists {
            assert!(p.id.as_str().starts_with("VL"));
            assert!(
                p.author.as_deref().is_some_and(|a| !LABELS.contains(&a)),
                "{p:?}"
            );
        }
        // Clips are in the response but not among the tracks; profiles are not artists.
        let clips = non_song_ids(&raw);
        assert!(!clips.is_empty());
        assert!(page.tracks.iter().all(|t| !clips.contains(&t.id.0)));
        let profiles: Vec<&str> = find_all(&raw, "browseEndpoint")
            .into_iter()
            .filter(|b| {
                find_first(b, "pageType").and_then(Value::as_str)
                    == Some("MUSIC_PAGE_TYPE_USER_CHANNEL")
            })
            .filter_map(|b| b.get("browseId").and_then(Value::as_str))
            .collect();
        assert!(
            page.artists
                .iter()
                .all(|a| !profiles.contains(&a.id.as_str()))
        );
    }

    #[test]
    fn all_kinds_with_album_card() {
        let page = parse_search(&fixture("search_all_album.json"), SearchKind::All).unwrap();
        let Some(SearchItem::Album(top)) = &page.top else {
            panic!("{:?}", page.top)
        };
        assert_eq!(top.id.as_str(), "MPREb_K8qWMWVqXGi");
        assert_eq!(top.title, "Random Access Memories");
        assert_eq!(top.year, Some(2013));
        assert_eq!(top.album_type, Some(AlbumType::Album));
        assert_eq!(top.artists.len(), 1);
        assert_eq!(top.artists[0].name, "Daft Punk");
        assert_eq!(
            top.artists[0].id.as_ref().map(|a| a.as_str()),
            Some("UCRr1xG_2WIDs18a6cIiCxeA")
        );
        assert!(
            page.albums.iter().all(|a| a.id != top.id),
            "the top card is not repeated"
        );
        // Rows of this layout name artists without links; the menu gives the first one its id.
        let t = &page.tracks[0];
        assert_eq!(t.artists[0].name, "Daft Punk");
        assert!(t.artists[0].id.is_some());
        for t in &page.tracks {
            no_labels(&t.artists);
        }
    }

    #[test]
    fn filtered_tracks_and_continuation() {
        let page = parse_search(&fixture("search_tracks.json"), SearchKind::Tracks).unwrap();
        assert!(page.top.is_none());
        assert_eq!(page.tracks.len(), 20);
        assert!(page.albums.is_empty() && page.artists.is_empty() && page.playlists.is_empty());
        assert!(page.continuation.is_some());
        let t = &page.tracks[0];
        assert_eq!(
            (t.id.as_str(), t.title.as_str()),
            ("w3XzgjlKFjc", "Draft Punk")
        );
        assert_eq!(t.artists.len(), 1);
        assert_eq!(t.artists[0].name, "DJT");
        assert!(t.artists[0].id.is_some());
        assert_eq!(t.album.as_ref().map(|a| a.title.as_str()), Some("Q-ENA II"));
        assert_eq!(t.duration_ms, 208_000);
        assert!(t.thumbnail.is_some());
        for t in &page.tracks {
            no_labels(&t.artists);
            assert!(t.duration_ms > 0 && t.album.is_some(), "{t:?}");
        }
        let get_lucky = page
            .tracks
            .iter()
            .find(|t| t.id.as_str() == "mrTQ0Buu2Us")
            .unwrap();
        let names: Vec<&str> = get_lucky.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Daft Punk", "Pharrell Williams", "Nile Rodgers"]);

        let more = parse_more(&fixture("search_tracks_more.json"), SearchKind::Tracks);
        assert_eq!(more.tracks.len(), 20);
        assert!(more.continuation.is_some());
        assert_ne!(more.continuation, page.continuation);
        assert_eq!(more.tracks[0].id.as_str(), "yahl-2y4Vvo");
        assert_eq!(more.tracks[0].duration_ms, 624_000);
        // The end of the list: no continuation contents at all.
        assert_eq!(
            parse_more(&json!({}), SearchKind::Tracks),
            SearchPage::default()
        );
    }

    #[test]
    fn filtered_albums() {
        let page = parse_search(&fixture("search_albums.json"), SearchKind::Albums).unwrap();
        assert_eq!(page.albums.len(), 20);
        assert!(page.continuation.is_some());
        let a = &page.albums[0];
        assert_eq!(
            (a.id.as_str(), a.title.as_str()),
            ("MPREb_qOombZf3hfT", "daft punk 2")
        );
        assert_eq!(a.album_type, Some(AlbumType::Single));
        assert_eq!(a.year, Some(2025));
        assert_eq!(a.artists.len(), 1);
        assert_eq!(a.artists[0].name, "leonleon");
        assert!(a.artists[0].id.is_some());
        for a in &page.albums {
            no_labels(&a.artists);
            assert!(
                a.album_type.is_some() && a.year.is_some() && a.thumbnail.is_some(),
                "{a:?}"
            );
        }
    }

    #[test]
    fn filtered_artists() {
        let page = parse_search(&fixture("search_artists.json"), SearchKind::Artists).unwrap();
        assert_eq!(page.artists.len(), 7);
        assert!(page.continuation.is_none());
        assert_eq!(page.artists[0].id.as_str(), "UCRr1xG_2WIDs18a6cIiCxeA");
        assert_eq!(page.artists[0].name, "Daft Punk");
        assert!(
            page.artists
                .iter()
                .all(|a| a.id.as_str().starts_with("UC") && a.thumbnail.is_some())
        );
    }

    #[test]
    fn filtered_playlists() {
        let page = parse_search(&fixture("search_playlists.json"), SearchKind::Playlists).unwrap();
        assert_eq!(page.playlists.len(), 20);
        assert!(page.continuation.is_some());
        let p = &page.playlists[0];
        assert!(p.id.as_str().starts_with("VLPL"));
        assert_eq!(p.author.as_deref(), Some("Funkonaut Sound"));
        assert_eq!(p.track_count, None, "views are not a track count");
        assert!(
            page.playlists
                .iter()
                .all(|p| p.author.is_some() && p.thumbnail.is_some())
        );
    }

    #[test]
    fn suggestions() {
        let s = parse_suggestions(&fixture("search_suggest.json"));
        assert_eq!(
            s,
            [
                "daft punk get lucky",
                "daft punk veridis quo",
                "daft punk giorgio moroder",
                "daft punk end of line",
                "daft punk around the world"
            ]
        );
        // History comes first; repeats and entity rows are dropped.
        let resp = json!({"contents": [{"searchSuggestionsSectionRenderer": {"contents": [
            {"historySuggestionRenderer": {"navigationEndpoint": {"searchEndpoint": {"query": "madeon"}}}},
            {"searchSuggestionRenderer": {"suggestion": {"runs": [{"text": "made", "bold": true}, {"text": "on"}]}}},
            {"searchSuggestionRenderer": {"navigationEndpoint": {"searchEndpoint": {"query": "madeon icarus"}}}},
            {"musicResponsiveListItemRenderer": {}}
        ]}}]});
        assert_eq!(parse_suggestions(&resp), ["madeon", "madeon icarus"]);
    }

    fn row(title: Value, sub: Vec<Value>, extra: Value) -> Value {
        let mut r = json!({"flexColumns": [
            {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [title]}}},
            {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": sub}}}
        ]});
        if let (Value::Object(r), Value::Object(e)) = (&mut r, extra) {
            r.extend(e);
        }
        json!({"musicResponsiveListItemRenderer": r})
    }

    fn browse(id: &str, page_type: &str) -> Value {
        json!({"browseEndpoint": {"browseId": id,
            "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": page_type}}}})
    }

    #[test]
    fn rows_by_endpoint() {
        let dot = json!({"text": " • "});
        let editorial = row(
            json!({"text": "Presenting Daft Punk"}),
            vec![
                json!({"text": "Плейлист"}),
                dot.clone(),
                json!({"text": "YouTube Music"}),
                dot.clone(),
                json!({"text": "35 треков"}),
            ],
            json!({"navigationEndpoint": browse("VLRDCLAK5uy_x", "MUSIC_PAGE_TYPE_PLAYLIST")}),
        );
        let Some(SearchItem::Playlist(p)) = row_item(&editorial, true, None) else {
            panic!()
        };
        assert_eq!(p.author.as_deref(), Some("YouTube Music"));
        assert_eq!(p.track_count, Some(35));
        let profile = row(
            json!({"text": "someone"}),
            vec![
                json!({"text": "Профиль"}),
                dot.clone(),
                json!({"text": "@someone"}),
            ],
            json!({"navigationEndpoint": browse("UCx", "MUSIC_PAGE_TYPE_USER_CHANNEL")}),
        );
        assert!(row_item(&profile, true, None).is_none());
        let episode = row(
            json!({"text": "Ep", "navigationEndpoint": {"watchEndpoint": {"videoId": "eeeeeeeeeee",
                "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": "MUSIC_VIDEO_TYPE_PODCAST_EPISODE"}}}}}),
            vec![
                json!({"text": "Выпуск"}),
                dot.clone(),
                json!({"text": "Show"}),
            ],
            json!({"playlistItemData": {"videoId": "eeeeeeeeeee"}}),
        );
        assert!(row_item(&episode, true, None).is_none());
        // An untyped song row whose first artist has no link keeps the name.
        let song = row(
            json!({"text": "Song", "navigationEndpoint": {"watchEndpoint": {"videoId": "sssssssssss"}}}),
            vec![
                json!({"text": "Various Artists"}),
                dot.clone(),
                json!({"text": "3:07"}),
            ],
            json!({"playlistItemData": {"videoId": "sssssssssss"}}),
        );
        let Some(SearchItem::Track(t)) = row_item(&song, false, None) else {
            panic!()
        };
        assert_eq!(t.artists.len(), 1);
        assert_eq!(t.artists[0].name, "Various Artists");
        assert_eq!(t.duration_ms, 187_000);
        assert_eq!(t.kind, VideoKind::Unknown);
    }

    #[test]
    fn shelf_labels() {
        let dot = json!({"text": " • "});
        let typed = |first: &str| {
            row(
                json!({"text": "x"}),
                vec![json!({"text": first}), dot.clone(), json!({"text": "y"})],
                json!({}),
            )
        };
        assert!(labelled(&[typed("Исполнитель"), typed("Исполнитель")]));
        assert!(!labelled(&[typed("Сингл"), typed("Альбом")]));
        assert!(!labelled(&[typed("Исполнитель")]), "one row says nothing");
        let linked = row(
            json!({"text": "x"}),
            vec![
                json!({"text": "DJT", "navigationEndpoint": browse("UCd", "MUSIC_PAGE_TYPE_ARTIST")}),
                dot.clone(),
                json!({"text": "3:00"}),
            ],
            json!({}),
        );
        assert!(!labelled(&[linked.clone(), linked]));
    }

    #[test]
    fn kind_filter_and_dedup() {
        let track = |id: &str| YtTrack {
            id: TrackId::new(id),
            title: id.into(),
            artists: vec![],
            album: None,
            duration_ms: 0,
            thumbnail: None,
            kind: VideoKind::Audio,
            track_no: None,
        };
        let artist = YtArtistCard {
            id: ArtistId::new("UCa"),
            name: "A".into(),
            thumbnail: None,
        };
        let page = SearchPage {
            top: Some(SearchItem::Track(track("t1"))),
            tracks: vec![track("t1"), track("t2"), track("t2")],
            artists: vec![artist],
            ..SearchPage::default()
        };
        let all = page.clone().finish(SearchKind::All);
        assert_eq!(
            all.tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["t2"]
        );
        assert_eq!(all.artists.len(), 1);
        let only = page.finish(SearchKind::Artists);
        assert!(only.top.is_none() && only.tracks.is_empty());
        assert_eq!(only.artists.len(), 1);
    }

    #[test]
    fn request_bodies() {
        assert_eq!(
            search_body("daft punk", SearchKind::All),
            json!({"query": "daft punk"})
        );
        let b = search_body("daft punk", SearchKind::Albums);
        assert_eq!(b["params"], "EgWKAQIYAWoSEAUQAxAJEAoQBBAQEBUQDhAR");
        assert_eq!(more_body("C1"), json!({"continuation": "C1"}));
        assert_eq!(suggest_body("daft p"), json!({"input": "daft p"}));
        let kinds = [
            SearchKind::Tracks,
            SearchKind::Albums,
            SearchKind::Artists,
            SearchKind::Playlists,
        ];
        let params: HashSet<&str> = kinds.iter().filter_map(|k| k.params()).collect();
        assert_eq!(params.len(), 4);
    }
}
