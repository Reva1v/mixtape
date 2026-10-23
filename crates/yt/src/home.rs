//! The YouTube Music home page (`browse FEmusic_home`): carousels of tracks, albums,
//! playlists, artists and mixes. The first page holds a few shelves, the rest come through
//! continuations. Shelf titles are localized (hl=ru), so detection relies only on renderer
//! names, endpoints and id prefixes.

use crate::api::{YtAlbumCard, YtArtistCard, YtPlaylistCard, YtTrack};
use crate::innertube::InnerTube;
use crate::parse::{self, at, find_all, find_first, runs, text};
use crate::{Result, YtError};
use mixtape_core::{ArtistRef, PlaylistId, TrackId, tr};
use serde_json::Value;

/// Upper bound on continuation pages of the home feed.
const MAX_HOME_PAGES: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub enum HomeItem {
    /// A song or a music video (any `VideoKind`: the caller decides what to show).
    Track(YtTrack),
    Album(YtAlbumCard),
    Playlist(YtPlaylistCard),
    Artist(YtArtistCard),
    /// An endless radio queue (`RD…`), played through `next` rather than browsed.
    Mix {
        playlist_id: String,
        title: String,
        subtitle: Option<String>,
        thumbnail: Option<String>,
        /// The video the mix starts from, when the card names one.
        seed: Option<TrackId>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HomeShelf {
    pub title: String,
    pub items: Vec<HomeItem>,
}

/// One page of the home feed: its shelves and the token for the next page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomePage {
    pub shelves: Vec<HomeShelf>,
    pub continuation: Option<String>,
}

/// A filter chip above the home feed («Релакс», «Энергия»): the feed for a mood is the
/// same browse with the chip's params.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeChip {
    pub title: String,
    pub params: String,
    pub selected: bool,
}

/// The home feed with its chips (they come with the first page only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomeFeed {
    pub chips: Vec<HomeChip>,
    pub shelves: Vec<HomeShelf>,
}

/// The chip cloud of a first page; chips without params (none to browse) are skipped.
pub fn chips(resp: &Value) -> Vec<HomeChip> {
    find_all(resp, "chipCloudChipRenderer")
        .into_iter()
        .filter_map(|c| {
            let params = at(c, &["navigationEndpoint", "browseEndpoint", "params"])?.as_str()?;
            let title = text(c.get("text")).trim().to_owned();
            (!title.is_empty() && !params.is_empty()).then(|| HomeChip {
                title,
                params: params.to_owned(),
                selected: c
                    .get("isSelected")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// A `browse` response (first page or continuation) → shelves. Shelves without a single
/// recognized item are dropped.
pub fn parse_page(resp: &Value) -> HomePage {
    let Some(section) = find_first(resp, "sectionListRenderer")
        .or_else(|| find_first(resp, "sectionListContinuation"))
    else {
        return HomePage::default();
    };
    let shelves = section
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(shelf)
        .collect();
    let continuation = section
        .get("continuations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|c| {
            c.get("nextContinuationData")
                .or_else(|| c.get("reloadContinuationData"))?
                .get("continuation")?
                .as_str()
        })
        .map(str::to_owned);
    HomePage {
        shelves,
        continuation,
    }
}

/// A carousel section → shelf; `None` for other sections (chips, tastebuilder) and for
/// carousels left empty after parsing.
pub fn shelf(section: &Value) -> Option<HomeShelf> {
    // A plain list shelf (chart rows) has its title on the renderer itself.
    if let Some(list) = section.get("musicShelfRenderer") {
        let items: Vec<HomeItem> = list.get("contents").and_then(Value::as_array).into_iter().flatten().filter_map(item).collect();
        let title = text(list.get("title")).trim().to_owned();
        return (!items.is_empty()).then_some(HomeShelf { title, items });
    }
    let carousel = section
        .get("musicCarouselShelfRenderer")
        .or_else(|| section.get("musicImmersiveCarouselShelfRenderer"))?;
    let header = carousel.get("header");
    let title = header
        .and_then(|h| at(h, &["musicCarouselShelfBasicHeaderRenderer", "title"]))
        .or_else(|| header.and_then(|h| find_first(h, "title")))
        .map(|t| text(Some(t)))
        .unwrap_or_default()
        .trim()
        .to_owned();
    let items: Vec<HomeItem> = carousel
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(item)
        .collect();
    (!items.is_empty()).then_some(HomeShelf { title, items })
}

/// One carousel entry: a song row or a card.
pub fn item(entry: &Value) -> Option<HomeItem> {
    if let Some(row) = entry.get("musicResponsiveListItemRenderer") {
        // Quick picks put "Artist • 12 млн прослушиваний" in the second column.
        let mut track = parse::list_item_track(row)?;
        let byline = at(
            row,
            &[
                "flexColumns",
                "1",
                "musicResponsiveListItemFlexColumnRenderer",
                "text",
            ],
        );
        track.artists = artist_segment(runs(byline));
        return Some(HomeItem::Track(track));
    }
    let card = entry.get("musicTwoRowItemRenderer")?;
    let nav = card.get("navigationEndpoint")?;
    // A card with a video is a song, even when it plays inside its radio (`RDAMVM…`).
    if let Some(watch) = nav.get("watchEndpoint") {
        let playlist = watch.get("playlistId").and_then(Value::as_str);
        return match (watch.get("videoId").and_then(Value::as_str), playlist) {
            (Some(video), _) => two_row_track(card, video).map(HomeItem::Track),
            (None, Some(pl)) if pl.starts_with("RD") => Some(mix(card, pl, None)),
            _ => None,
        };
    }
    if let Some(pl) = at(nav, &["watchPlaylistEndpoint", "playlistId"]).and_then(Value::as_str) {
        return Some(if pl.starts_with("RD") && !pl.starts_with("RDCLAK") {
            mix(card, pl, None)
        } else {
            HomeItem::Playlist(playlist_card(card, &format!("VL{pl}"))?)
        });
    }
    let id = parse::browse_id(card)?;
    if id.starts_with("MPRE") {
        parse::two_row_album(card).map(HomeItem::Album)
    } else if id.starts_with("UC") {
        parse::two_row_artist(card).map(HomeItem::Artist)
    } else if id.starts_with("VLRD") && !id.starts_with("VLRDCLAK") {
        Some(mix(card, &id[2..], None))
    } else if id.starts_with("VL") {
        parse::two_row_playlist(card).map(HomeItem::Playlist)
    } else if id.starts_with("PL") || id.starts_with("RDCLAK") || id.starts_with("OLAK") {
        playlist_card(card, &format!("VL{id}")).map(HomeItem::Playlist)
    } else {
        None
    }
}

/// A playlist card whose endpoint lacks the `VL` browse id: parse it as if it had one.
fn playlist_card(card: &Value, browse_id: &str) -> Option<YtPlaylistCard> {
    let mut p = parse::two_row_playlist(card).or_else(|| {
        let mut fake = card.clone();
        fake["navigationEndpoint"] = serde_json::json!({"browseEndpoint": {"browseId": browse_id}});
        if let Some(run) = fake
            .pointer_mut("/title/runs/0")
            .and_then(Value::as_object_mut)
        {
            run.remove("navigationEndpoint");
        }
        parse::two_row_playlist(&fake)
    })?;
    if !p.id.as_str().starts_with("VL") {
        p.id = PlaylistId::new(browse_id);
    }
    Some(p)
}

fn mix(card: &Value, playlist_id: &str, seed: Option<&str>) -> HomeItem {
    let subtitle = text(card.get("subtitle")).trim().to_owned();
    HomeItem::Mix {
        playlist_id: playlist_id.to_owned(),
        title: text(card.get("title")).trim().to_owned(),
        subtitle: (!subtitle.is_empty()).then_some(subtitle),
        thumbnail: card.get("thumbnailRenderer").and_then(parse::thumbnail),
        seed: seed.map(TrackId::new),
    }
}

/// Artists from a byline of " • "-separated parts, in any language: "Композиция • Artist",
/// "Artist и Artist • 53 млн просмотров". The part with a linked artist wins, else the first.
fn artist_segment(byline: &[Value]) -> Vec<ArtistRef> {
    let is_dot = |r: &Value| r.get("text").and_then(Value::as_str).map(str::trim) == Some("•");
    let mut parts = byline.split(|r| is_dot(r)).filter(|p| !p.is_empty());
    let first = parts.clone().next().unwrap_or(&[]);
    let linked = parts.find(|p| {
        p.iter().any(|r| {
            matches!(
                parse::page_type(r),
                Some("MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL")
            )
        })
    });
    parse::artists_from_runs(linked.unwrap_or(first))
}

/// A song or video card ("Послушать ещё раз", "Видеоклипы для вас").
fn two_row_track(card: &Value, video_id: &str) -> Option<YtTrack> {
    Some(YtTrack {
        id: TrackId::new(video_id),
        title: text(card.get("title")).trim().to_owned(),
        artists: artist_segment(runs(card.get("subtitle"))),
        album: None,
        duration_ms: 0,
        thumbnail: card.get("thumbnailRenderer").and_then(parse::thumbnail),
        kind: parse::video_kind(card.get("navigationEndpoint")?),
        track_no: None,
    })
}

/// A mood or genre tile of `FEmusic_moods_and_genres`: opens
/// `FEmusic_moods_and_genres_category` with its params.
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub title: String,
    pub params: String,
    /// The tile's accent (`solid.leftStripeColor`, ARGB), the stripe on its left in YouTube Music.
    pub color: Option<u32>,
}

/// A titled group of tiles («Настроения и события», «Жанры»).
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryGroup {
    pub title: String,
    pub items: Vec<Category>,
}

/// The tile grids of the moods-and-genres page; tiles without params are skipped.
pub fn categories(resp: &Value) -> Vec<CategoryGroup> {
    find_all(resp, "gridRenderer")
        .into_iter()
        .filter_map(|grid| {
            let title = grid.get("header").and_then(|h| find_first(h, "title")).map(|t| text(Some(t))).unwrap_or_default().trim().to_owned();
            let items: Vec<Category> = grid
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|i| {
                    let b = i.get("musicNavigationButtonRenderer")?;
                    let params = at(b, &["clickCommand", "browseEndpoint", "params"])?.as_str()?;
                    let title = text(b.get("buttonText")).trim().to_owned();
                    let color = at(b, &["solid", "leftStripeColor"]).and_then(stripe_color);
                    (!title.is_empty()).then(|| Category { title, params: params.to_owned(), color })
                })
                .collect();
            (!items.is_empty()).then_some(CategoryGroup { title, items })
        })
        .collect()
}

/// An ARGB colour as InnerTube writes it: a number, now and then a numeric string.
fn stripe_color(v: &Value) -> Option<u32> {
    let n = v.as_u64().or_else(|| v.as_i64().map(|n| n as u32 as u64)).or_else(|| v.as_str()?.parse().ok())?;
    u32::try_from(n).ok()
}

impl InnerTube {
    /// Shelves of any browse page built like the home feed (charts, new releases, a mood
    /// category), following continuations up to `max_shelves`.
    pub async fn browse_shelves(&self, browse_id: &str, params: Option<&str>, max_shelves: usize) -> Result<Vec<HomeShelf>> {
        let first = match params {
            Some(p) => self.browse_params(browse_id, p).await?,
            None => self.browse(browse_id).await?,
        };
        let mut page = parse_page(&first);
        let mut shelves = std::mem::take(&mut page.shelves);
        let mut pages = 1;
        while shelves.len() < max_shelves
            && pages < MAX_HOME_PAGES
            && let Some(token) = page.continuation.take()
        {
            page = parse_page(&self.browse_continuation(&token).await?);
            shelves.append(&mut page.shelves);
            pages += 1;
        }
        shelves.truncate(max_shelves);
        Ok(shelves)
    }

    /// The tiles of `FEmusic_moods_and_genres`.
    pub async fn mood_categories(&self) -> Result<Vec<CategoryGroup>> {
        Ok(categories(&self.browse("FEmusic_moods_and_genres").await?))
    }

    /// `browse FEmusic_home`, then its continuations until `max_shelves` shelves are
    /// collected or the feed ends.
    pub async fn home(&self, max_shelves: usize) -> Result<Vec<HomeShelf>> {
        Ok(self.home_feed(None, max_shelves).await?.shelves)
    }

    /// The home feed and its chips; with `chip` (a chip's params) the feed for that mood.
    pub async fn home_feed(&self, chip: Option<&str>, max_shelves: usize) -> Result<HomeFeed> {
        let first = match chip {
            Some(params) => self.browse_params("FEmusic_home", params).await?,
            None => self.browse("FEmusic_home").await?,
        };
        let chips = chips(&first);
        let mut page = parse_page(&first);
        let mut shelves = std::mem::take(&mut page.shelves);
        let mut pages = 1;
        while shelves.len() < max_shelves
            && pages < MAX_HOME_PAGES
            && let Some(token) = page.continuation.take()
        {
            page = parse_page(&self.browse_continuation(&token).await?);
            shelves.append(&mut page.shelves);
            pages += 1;
        }
        shelves.truncate(max_shelves);
        if shelves.is_empty() && max_shelves > 0 {
            return Err(YtError::Parse(
                tr("the home page has no shelves", "на головній немає жодної добірки", "на главной нет ни одной подборки").into(),
            ));
        }
        Ok(HomeFeed { chips, shelves })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::VideoKind;
    use crate::parse::find_all;
    use serde_json::json;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn kinds(shelf: &HomeShelf) -> Vec<&'static str> {
        let mut out: Vec<&str> = shelf
            .items
            .iter()
            .map(|i| match i {
                HomeItem::Track(t) if t.kind == VideoKind::Audio => "track",
                HomeItem::Track(_) => "video",
                HomeItem::Album(_) => "album",
                HomeItem::Playlist(_) => "playlist",
                HomeItem::Artist(_) => "artist",
                HomeItem::Mix { .. } => "mix",
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn raw_items(resp: &Value) -> usize {
        find_all(resp, "musicTwoRowItemRenderer").len()
            + find_all(resp, "musicResponsiveListItemRenderer").len()
    }

    fn shelf<'a>(page: &'a HomePage, title: &str) -> &'a HomeShelf {
        let titles: Vec<&str> = page.shelves.iter().map(|s| s.title.as_str()).collect();
        page.shelves
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("no «{title}» in {titles:?}"))
    }

    #[test]
    fn list_shelves_and_category_tiles() {
        let list = json!({ "musicShelfRenderer": {
            "title": { "runs": [{ "text": "Топ" }] },
            "contents": [{ "musicResponsiveListItemRenderer": {
                "flexColumns": [
                    { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{
                        "text": "Song", "navigationEndpoint": { "watchEndpoint": { "videoId": "vid00000001",
                        "watchEndpointMusicSupportedConfigs": { "watchEndpointMusicConfig": { "musicVideoType": "MUSIC_VIDEO_TYPE_ATV" } } } } }] } } },
                    { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "Artist" }] } } }
                ]
            } }]
        } });
        let got = super::shelf(&list).expect("a list shelf");
        assert_eq!(got.title, "Топ");
        assert_eq!(got.items.len(), 1);

        let page = json!({ "contents": [{ "gridRenderer": {
            "header": { "gridHeaderRenderer": { "title": { "runs": [{ "text": "Жанры" }] } } },
            "items": [
                { "musicNavigationButtonRenderer": { "buttonText": { "runs": [{ "text": "Рок" }] },
                  "solid": { "leftStripeColor": 4294901760_u64 },
                  "clickCommand": { "browseEndpoint": { "browseId": "FEmusic_moods_and_genres_category", "params": "ggMProck" } } } },
                { "musicNavigationButtonRenderer": { "buttonText": { "runs": [{ "text": "Джаз" }] },
                  "solid": { "leftStripeColor": "4278255360" },
                  "clickCommand": { "browseEndpoint": { "params": "ggMPjazz" } } } },
                { "musicNavigationButtonRenderer": { "buttonText": { "runs": [{ "text": "Блюз" }] },
                  "clickCommand": { "browseEndpoint": { "params": "ggMPblues" } } } },
                { "musicNavigationButtonRenderer": { "buttonText": { "runs": [{ "text": "Пусто" }] } } }
            ]
        } }] });
        let tile = |title: &str, params: &str, color| Category { title: title.into(), params: params.into(), color };
        assert_eq!(
            categories(&page),
            vec![CategoryGroup {
                title: "Жанры".into(),
                items: vec![
                    tile("Рок", "ggMProck", Some(0xFFFF_0000)),
                    tile("Джаз", "ggMPjazz", Some(0xFF00_FF00)),
                    tile("Блюз", "ggMPblues", None),
                ],
            }]
        );
    }

    #[test]
    fn chips_carry_their_params() {
        let page = json!({ "header": { "chipCloudRenderer": { "chips": [
            { "chipCloudChipRenderer": {
                "text": { "runs": [{ "text": "Релакс" }] },
                "navigationEndpoint": { "browseEndpoint": { "browseId": "FEmusic_home", "params": "ggMPOg1" } },
                "isSelected": true
            } },
            { "chipCloudChipRenderer": {
                "text": { "runs": [{ "text": "Энергия" }] },
                "navigationEndpoint": { "browseEndpoint": { "browseId": "FEmusic_home", "params": "ggMPOg2" } }
            } },
            { "chipCloudChipRenderer": {
                "text": { "runs": [{ "text": "Без параметров" }] },
                "navigationEndpoint": { "browseEndpoint": { "browseId": "FEmusic_home" } }
            } }
        ] } } });
        let got = chips(&page);
        assert_eq!(
            got,
            vec![
                HomeChip {
                    title: "Релакс".into(),
                    params: "ggMPOg1".into(),
                    selected: true
                },
                HomeChip {
                    title: "Энергия".into(),
                    params: "ggMPOg2".into(),
                    selected: false
                },
            ]
        );
        // The sanitized fixture's chips have no params: nothing to browse, nothing shown.
        assert!(chips(&fixture("browse_home.json")).is_empty());
    }

    #[test]
    fn first_page() {
        let raw = fixture("browse_home.json");
        let page = parse_page(&raw);
        let titles: Vec<&str> = page.shelves.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Shelf 1", "Shelf 2", "Shelf 3"]);
        assert!(page.continuation.is_some());
        let parsed: usize = page.shelves.iter().map(|s| s.items.len()).sum();
        assert_eq!(parsed, raw_items(&raw), "every card is recognized");

        // Songs played inside their radio (`RDAMVM…`) are tracks, not mixes.
        let again = shelf(&page, "Shelf 1");
        assert_eq!(kinds(again), ["playlist", "track", "video"]);
        let HomeItem::Track(song) = &again.items[0] else {
            panic!("{:?}", again.items[0])
        };
        assert_eq!(song.kind, VideoKind::Audio);
        assert_eq!(song.id.as_str(), "v0000000001");
        assert!(song.title.starts_with("Title "), "{}", song.title);
        // "Композиция • Artist 1": the type label is not an artist.
        assert_eq!(song.artists.len(), 1);
        assert!(
            song.artists[0].name.starts_with("Artist "),
            "{:?}",
            song.artists
        );
        let artist_id = song.artists[0].id.as_ref().map(|a| a.as_str());
        assert!(
            artist_id.is_some_and(|a| a.starts_with("UCsyn")),
            "{:?}",
            song.artists
        );
        assert!(song.thumbnail.is_some());
        let liked = again.items.iter().find_map(|i| match i {
            HomeItem::Playlist(p) => Some(p),
            _ => None,
        });
        assert_eq!(liked.unwrap().id.as_str(), "VLLM");

        let all: Vec<&HomeItem> = page.shelves.iter().flat_map(|s| &s.items).collect();
        assert!(
            all.iter()
                .any(|i| matches!(i, HomeItem::Artist(a) if a.id.as_str().starts_with("UC")))
        );
        for i in all {
            if let HomeItem::Playlist(p) = i {
                assert!(
                    p.id.as_str().starts_with("VL") && !p.title.is_empty(),
                    "{p:?}"
                );
            }
        }
    }

    #[test]
    fn continuation_with_mixes() {
        let raw = fixture("browse_home_more.json");
        let page = parse_page(&raw);
        let titles: Vec<&str> = page.shelves.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Shelf 4", "Shelf 5", "Shelf 6"]);
        assert!(page.continuation.is_some());
        let parsed: usize = page.shelves.iter().map(|s| s.items.len()).sum();
        assert_eq!(parsed, raw_items(&raw));
        let mixes: Vec<&HomeItem> = page
            .shelves
            .iter()
            .flat_map(|s| &s.items)
            .filter(|i| matches!(i, HomeItem::Mix { .. }))
            .collect();
        assert!(!mixes.is_empty());
        for m in mixes {
            let HomeItem::Mix {
                playlist_id,
                title,
                subtitle,
                thumbnail,
                seed,
            } = m
            else {
                unreachable!()
            };
            assert!(playlist_id.starts_with("RDTMAK5uy_syn"), "{playlist_id}");
            assert!(title.starts_with("Playlist "), "{title}");
            assert!(subtitle.is_some() && thumbnail.is_some());
            assert!(seed.is_none());
        }
        // Videos keep their kind: the caller decides whether to show them.
        let videos = page
            .shelves
            .iter()
            .flat_map(|s| &s.items)
            .filter(|i| matches!(i, HomeItem::Track(t) if t.kind == VideoKind::Video));
        assert!(videos.count() > 0);
    }

    #[test]
    fn quick_picks_rows() {
        let raw = fixture("browse_home_rows.json");
        let page = parse_page(&raw);
        let titles: Vec<&str> = page.shelves.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Shelf 7", "Shelf 8", "Shelf 9"]);
        assert!(page.continuation.is_some());
        let parsed: usize = page.shelves.iter().map(|s| s.items.len()).sum();
        assert_eq!(parsed, raw_items(&raw));
        let rows = page
            .shelves
            .iter()
            .find(|s| kinds(s) == ["track"] && s.items.len() >= 10)
            .expect("a shelf of song rows");
        for i in &rows.items {
            let HomeItem::Track(t) = i else {
                unreachable!()
            };
            assert!(t.id.as_str().len() == 11 && t.id.as_str().starts_with('v'));
            assert!(t.title.starts_with("Track "), "{t:?}");
            // "Artist 5 • 12 млн прослушиваний": the play count is not an artist.
            assert!(!t.artists.is_empty(), "{t:?}");
            assert!(
                t.artists
                    .iter()
                    .all(|a| a.id.is_some() && a.name.starts_with("Artist ")),
                "{t:?}"
            );
            let album = t.album.as_ref().map(|a| a.id.as_str());
            assert!(album.is_some_and(|a| a.starts_with("MPREb_syn")), "{t:?}");
        }
        let clips = shelf(&page, "Shelf 7");
        assert_eq!(kinds(clips), ["video"]);
    }

    #[test]
    fn mix_and_playlist_ids() {
        let card = |nav: Value| {
            json!({"musicTwoRowItemRenderer": {
                "title": {"runs": [{"text": "Мой микс 1"}]},
                "subtitle": {"runs": [{"text": "A, B, C"}]},
                "navigationEndpoint": nav}})
        };
        let browse = |id: &str| card(json!({"browseEndpoint": {"browseId": id}}));
        let mix_id = |i: Option<HomeItem>| match i {
            Some(HomeItem::Mix { playlist_id, .. }) => playlist_id,
            other => panic!("{other:?}"),
        };
        assert_eq!(mix_id(item(&browse("VLRDTMAK5uy_x"))), "RDTMAK5uy_x");
        let watch = card(json!({"watchPlaylistEndpoint": {"playlistId": "RDTMAK5uy_y"}}));
        assert_eq!(mix_id(item(&watch)), "RDTMAK5uy_y");
        let bare = card(json!({"watchEndpoint": {"playlistId": "RDEMabc"}}));
        assert_eq!(mix_id(item(&bare)), "RDEMabc");

        for (id, want) in [
            ("VLRDCLAK5uy_z", "VLRDCLAK5uy_z"),
            ("RDCLAK5uy_z", "VLRDCLAK5uy_z"),
            ("PLabc", "VLPLabc"),
        ] {
            match item(&browse(id)) {
                Some(HomeItem::Playlist(p)) => assert_eq!(p.id.as_str(), want),
                other => panic!("{id}: {other:?}"),
            }
        }
        let curated = card(json!({"watchPlaylistEndpoint": {"playlistId": "RDCLAK5uy_q"}}));
        assert!(
            matches!(item(&curated), Some(HomeItem::Playlist(p)) if p.id.as_str() == "VLRDCLAK5uy_q")
        );
        assert!(item(&browse("FEmusic_moods_and_genres")).is_none());
    }

    #[test]
    fn skips_empty_and_foreign_sections() {
        let resp = json!({"contents": {"sectionListRenderer": {"contents": [
            {"musicCarouselShelfRenderer": {
                "header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Пусто"}]}}},
                "contents": [{"musicTwoRowItemRenderer": {"title": {"runs": [{"text": "?"}]},
                    "navigationEndpoint": {"browseEndpoint": {"browseId": "FEmusic_x"}}}}]}},
            {"musicTastebuilderShelfRenderer": {}},
        ]}}});
        let page = parse_page(&resp);
        assert!(page.shelves.is_empty());
        assert!(page.continuation.is_none());
        assert_eq!(parse_page(&json!({})), HomePage::default());
    }
}
