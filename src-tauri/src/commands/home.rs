//! Home screen (FR-024, T086). The local part comes from the database and answers at once
//! (`home_get`); YouTube Music shelves come through their own call (`home_shelves`), so the
//! screen renders without waiting for the network and the shelves stream in after it.

use super::mix::{page_of, yt_error};
use super::{CmdResult, blocking};
use crate::state::AppState;
use mixtape_core::{
    Album, AlbumId, AppError, ArtistId, ArtistRef, ErrorKind, PlaylistId, Track, TrackId,
};
use mixtape_library::repo;
use mixtape_sync::sources;
use mixtape_yt::api::YtTrack;
use mixtape_yt::home::HomeItem;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::State;
use ts_rs::TS;

type St<'a> = State<'a, Arc<AppState>>;

/// How long fetched shelves are shown before the feed is read again.
const SHELF_TTL: Duration = Duration::from_secs(30 * 60);
/// Shelves read from the feed (the first page and a few continuations).
const MAX_SHELVES: usize = 12;
/// Card thumbnails downloaded at once.
const COVER_PARALLEL: usize = 6;

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HomePage {
    pub liked_count: u32,
    pub recent: Vec<Album>,
    pub added: Vec<Album>,
    pub top: Vec<Track>,
}

/// The local part: recently played and recently added albums, most played tracks.
#[tauri::command]
pub async fn home_get(state: St<'_>) -> CmdResult<HomePage> {
    let st = state.inner().clone();
    blocking(move || {
        Ok(st.db.read(|c| {
            Ok(HomePage {
                liked_count: repo::liked_count(c)?,
                recent: repo::albums_by_ids(c, &repo::recently_played_album_ids(c, 12)?)?,
                added: repo::albums_by_ids(c, &repo::recently_added_album_ids(c, 12)?)?,
                top: repo::tracks_by_ids(c, &repo::most_played_track_ids(c, 10)?)?,
            })
        })?)
    })
    .await
}

/// A YouTube Music shelf: a localized title and its cards.
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HomeShelf {
    pub title: String,
    pub items: Vec<ShelfItem>,
}

/// One card of a shelf. Tracks are stored rows (`remote` until played), so they play and
/// have the usual track menu; the rest are light cards that open their page by id.
#[derive(Clone, Serialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ShelfItem {
    Track {
        track: Track,
    },
    Album {
        id: AlbumId,
        title: String,
        artists: Vec<ArtistRef>,
        #[ts(optional)]
        year: Option<u16>,
        #[ts(optional)]
        cover_key: Option<String>,
    },
    Playlist {
        id: PlaylistId,
        title: String,
        #[ts(optional)]
        subtitle: Option<String>,
        #[ts(optional)]
        cover_key: Option<String>,
    },
    Artist {
        id: ArtistId,
        name: String,
        #[ts(optional)]
        cover_key: Option<String>,
    },
    /// An endless radio: started with `mix_start_playlist`.
    Mix {
        playlist_id: String,
        title: String,
        #[ts(optional)]
        subtitle: Option<String>,
        #[ts(optional)]
        cover_key: Option<String>,
    },
}

/// A shelf as cached: tracks by id, re-read on every call so likes and states stay current.
#[derive(Clone)]
enum CachedItem {
    Track(TrackId),
    Card(ShelfItem),
}

#[derive(Clone)]
struct CachedShelf {
    title: String,
    items: Vec<CachedItem>,
}

struct Cache {
    at: Instant,
    shelves: Vec<CachedShelf>,
}

/// Held across a fetch, so concurrent calls wait for one request instead of starting more.
static CACHE: tokio::sync::Mutex<Option<Cache>> = tokio::sync::Mutex::const_new(None);

/// YouTube Music shelves, cached for 30 minutes (`force` reads the feed again). Empty when
/// offline, without an account, or when the feed fails and nothing is cached yet.
#[tauri::command]
pub async fn home_shelves(state: St<'_>, force: Option<bool>) -> CmdResult<Vec<HomeShelf>> {
    let st = state.inner().clone();
    if !st.yt.is_authenticated() {
        *CACHE.lock().await = None;
        return Ok(Vec::new());
    }
    if !st.online.load(Ordering::SeqCst) {
        return Ok(Vec::new());
    }
    let mut cache = CACHE.lock().await;
    let fresh = cache.as_ref().is_some_and(|c| c.at.elapsed() < SHELF_TTL);
    if force.unwrap_or(false) || !fresh {
        match fetch(&st).await {
            Ok(shelves) => {
                *cache = Some(Cache {
                    at: Instant::now(),
                    shelves,
                })
            }
            Err(e) => {
                tracing::warn!(error = %e.message, "home shelves unavailable");
                // Stale shelves are better than none, unless the account is gone.
                if e.kind == ErrorKind::Auth {
                    *cache = None;
                }
            }
        }
    }
    let shelves = cache
        .as_ref()
        .map(|c| c.shelves.clone())
        .unwrap_or_default();
    drop(cache);
    hydrate(st, shelves).await
}

/// Plays a mix card: the radio by its playlist id replaces the queue, like «Включить микс».
#[tauri::command]
pub async fn mix_start_playlist(state: St<'_>, playlist_id: String) -> CmdResult<()> {
    let st = state.inner().clone();
    if !st.yt.is_authenticated() {
        return Err(AppError::new(
            ErrorKind::Auth,
            "Миксы собирает YouTube Music — сначала подключите аккаунт",
        ));
    }
    let radio = st
        .yt
        .radio_playlist(&playlist_id)
        .await
        .map_err(|e| yt_error(&st, e))?;
    let page = page_of(&st, radio).await?;
    let Some(seed) = page.tracks.first().cloned() else {
        return Err(AppError::new(
            ErrorKind::NotFound,
            "YouTube Music не собрал этот микс",
        ));
    };
    blocking(move || {
        st.player.start_mix(seed, page);
        Ok(())
    })
    .await
}

/// Reads the feed, stores its tracks and downloads card thumbnails.
async fn fetch(st: &AppState) -> CmdResult<Vec<CachedShelf>> {
    let feed = st.yt.home(MAX_SHELVES).await.map_err(|e| yt_error(st, e))?;
    let tracks: Vec<YtTrack> = feed
        .iter()
        .flat_map(|s| &s.items)
        .filter_map(|i| match i {
            HomeItem::Track(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let mut seen = HashSet::new();
    let thumbs: Vec<(String, String)> = feed
        .iter()
        .flat_map(|s| &s.items)
        .filter_map(card_thumb)
        .filter(|(key, _)| seen.insert(key.clone()) && !st.covers.exists(key))
        .collect();
    let (stored, ()) = tokio::join!(
        sources::store_tracks(st.yt.http(), &st.db, &st.covers, tracks),
        download_thumbs(st, thumbs)
    );
    stored?;
    let cover = |key: &str| st.covers.exists(key).then(|| key.to_owned());
    Ok(feed
        .into_iter()
        .map(|s| {
            let mut keys = HashSet::new();
            let items = s
                .items
                .into_iter()
                .map(|i| cached(i, &cover))
                .filter(|i| keys.insert(item_key(i)))
                .collect();
            CachedShelf {
                title: s.title,
                items,
            }
        })
        .filter(|s| !s.items.is_empty())
        .collect())
}

/// The cover-store key and thumbnail URL of a card; tracks get theirs from `store_tracks`.
fn card_thumb(item: &HomeItem) -> Option<(String, String)> {
    let (key, url) = match item {
        HomeItem::Track(_) => return None,
        HomeItem::Album(a) => (a.id.to_string(), a.thumbnail.as_ref()),
        HomeItem::Playlist(p) => (p.id.to_string(), p.thumbnail.as_ref()),
        HomeItem::Artist(a) => (a.id.to_string(), a.thumbnail.as_ref()),
        HomeItem::Mix {
            playlist_id,
            thumbnail,
            ..
        } => (playlist_id.clone(), thumbnail.as_ref()),
    };
    Some((key, url?.clone()))
}

async fn download_thumbs(st: &AppState, jobs: Vec<(String, String)>) {
    let sem = Arc::new(tokio::sync::Semaphore::new(COVER_PARALLEL));
    let mut set = tokio::task::JoinSet::new();
    for (key, url) in jobs {
        let (http, covers, sem) = (st.yt.http().clone(), st.covers.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            let bytes = mixtape_yt::thumbs::download(&http, &url).await.ok()?;
            tokio::task::spawn_blocking(move || covers.process(&key, &bytes).ok())
                .await
                .ok()
                .flatten()
        });
    }
    while set.join_next().await.is_some() {}
}

fn cached(item: HomeItem, cover: &impl Fn(&str) -> Option<String>) -> CachedItem {
    CachedItem::Card(match item {
        HomeItem::Track(t) => return CachedItem::Track(t.id),
        HomeItem::Album(a) => ShelfItem::Album {
            cover_key: cover(a.id.as_str()),
            id: a.id,
            title: a.title,
            artists: a.artists,
            year: a.year,
        },
        HomeItem::Playlist(p) => ShelfItem::Playlist {
            cover_key: cover(p.id.as_str()),
            id: p.id,
            title: p.title,
            subtitle: p.author,
        },
        HomeItem::Artist(a) => ShelfItem::Artist {
            cover_key: cover(a.id.as_str()),
            id: a.id,
            name: a.name,
        },
        HomeItem::Mix {
            playlist_id,
            title,
            subtitle,
            ..
        } => ShelfItem::Mix {
            cover_key: cover(&playlist_id),
            playlist_id,
            title,
            subtitle,
        },
    })
}

/// Identity of a card within its shelf (the UI keys its lists by it).
fn item_key(item: &CachedItem) -> String {
    match item {
        CachedItem::Track(id) => format!("t:{id}"),
        CachedItem::Card(ShelfItem::Track { track }) => format!("t:{}", track.id),
        CachedItem::Card(ShelfItem::Album { id, .. }) => format!("a:{id}"),
        CachedItem::Card(ShelfItem::Playlist { id, .. }) => format!("p:{id}"),
        CachedItem::Card(ShelfItem::Artist { id, .. }) => format!("r:{id}"),
        CachedItem::Card(ShelfItem::Mix { playlist_id, .. }) => format!("m:{playlist_id}"),
    }
}

/// Cached shelves → UI shelves with fresh track rows; tracks missing from the database are
/// dropped, and so are shelves left empty.
async fn hydrate(st: Arc<AppState>, shelves: Vec<CachedShelf>) -> CmdResult<Vec<HomeShelf>> {
    if shelves.is_empty() {
        return Ok(Vec::new());
    }
    blocking(move || {
        let ids: Vec<TrackId> = shelves
            .iter()
            .flat_map(|s| &s.items)
            .filter_map(|i| match i {
                CachedItem::Track(id) => Some(id.clone()),
                CachedItem::Card(_) => None,
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let rows: HashMap<TrackId, Track> = st
            .db
            .read(|c| repo::tracks_by_ids(c, &ids))?
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();
        Ok(resolve(shelves, &rows))
    })
    .await
}

fn resolve(shelves: Vec<CachedShelf>, rows: &HashMap<TrackId, Track>) -> Vec<HomeShelf> {
    shelves
        .into_iter()
        .map(|s| HomeShelf {
            title: s.title,
            items: s
                .items
                .into_iter()
                .filter_map(|i| match i {
                    CachedItem::Track(id) => rows
                        .get(&id)
                        .cloned()
                        .map(|track| ShelfItem::Track { track }),
                    CachedItem::Card(card) => Some(card),
                })
                .collect(),
        })
        .filter(|s| !s.items.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_core::TrackState;

    fn track(id: &str) -> Track {
        Track {
            id: TrackId::new(id),
            title: id.to_owned(),
            artists: Vec::new(),
            album: None,
            track_no: None,
            duration_ms: 0,
            cover_key: None,
            state: TrackState::Remote,
            liked: false,
        }
    }

    fn mix(id: &str) -> HomeItem {
        HomeItem::Mix {
            playlist_id: id.to_owned(),
            title: "Mix".into(),
            subtitle: None,
            thumbnail: None,
            seed: None,
        }
    }

    #[test]
    fn cards_keep_covers_and_drop_duplicates() {
        let cover = |k: &str| (k == "RDA").then(|| k.to_owned());
        let items: Vec<CachedItem> = [mix("RDA"), mix("RDA"), mix("RDB")]
            .into_iter()
            .map(|i| cached(i, &cover))
            .collect();
        let mut keys = HashSet::new();
        let unique: Vec<String> = items
            .iter()
            .map(item_key)
            .filter(|k| keys.insert(k.clone()))
            .collect();
        assert_eq!(unique, ["m:RDA", "m:RDB"]);
        let CachedItem::Card(ShelfItem::Mix { cover_key, .. }) = &items[0] else {
            panic!("not a mix card")
        };
        assert_eq!(cover_key.as_deref(), Some("RDA"));
        let CachedItem::Card(ShelfItem::Mix { cover_key, .. }) = &items[2] else {
            panic!("not a mix card")
        };
        assert!(cover_key.is_none());
    }

    #[test]
    fn resolve_reads_tracks_and_drops_missing() {
        let shelves = vec![
            CachedShelf {
                title: "A".into(),
                items: vec![
                    CachedItem::Track(TrackId::new("t1")),
                    CachedItem::Track(TrackId::new("gone")),
                ],
            },
            CachedShelf {
                title: "Empty".into(),
                items: vec![CachedItem::Track(TrackId::new("gone"))],
            },
        ];
        let mut liked = track("t1");
        liked.liked = true;
        let rows = HashMap::from([(TrackId::new("t1"), liked)]);
        let out = resolve(shelves, &rows);
        assert_eq!(out.len(), 1, "a shelf left empty is dropped");
        assert_eq!(out[0].items.len(), 1);
        let ShelfItem::Track { track } = &out[0].items[0] else {
            panic!("not a track")
        };
        assert!(track.liked, "track rows come from the database");
    }
}
