//! The YouTube Music catalog (US4): search outside the library, artist pages from the
//! catalog and «Скачать» for albums, playlists and discographies.

use super::{CmdResult, blocking};
use crate::commands::library::emit_changed;
use crate::state::Session;
use mixtape_core::{
    Album, AlbumId, AppError, Artist, ArtistId, ErrorKind, Playlist, PlaylistId, Track, TrackId,
    tr, trf,
};
use mixtape_library::repo;
use mixtape_library::views::ViewKind;
use mixtape_sync::search::{CoverSink, HttpCovers, SearchError, SearchKey, Stored, TopKind};
use mixtape_sync::{catalog, sources, url_import};
use mixtape_yt::YtError;
use mixtape_yt::api::YtMusic;
use mixtape_yt::search::SearchKind;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;
use ts_rs::TS;

type St<'a> = State<'a, Arc<Session>>;

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CatalogKind {
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl From<CatalogKind> for SearchKind {
    fn from(k: CatalogKind) -> Self {
        match k {
            CatalogKind::All => SearchKind::All,
            CatalogKind::Tracks => SearchKind::Tracks,
            CatalogKind::Albums => SearchKind::Albums,
            CatalogKind::Artists => SearchKind::Artists,
            CatalogKind::Playlists => SearchKind::Playlists,
        }
    }
}

/// «Лучший результат».
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum CatalogTop {
    Track { track: Track },
    Album { album: Album },
    Artist { artist: Artist },
    Playlist { playlist: Playlist },
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CatalogPage {
    #[ts(optional)]
    pub top: Option<CatalogTop>,
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
    /// Albums with tracks in the library (shown as «скачано»).
    pub owned_albums: Vec<AlbumId>,
    #[ts(optional)]
    pub continuation: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArtistCatalog {
    pub top: Vec<Track>,
    pub albums: Vec<Album>,
}

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DownloadKind {
    Album,
    Artist,
    Playlist,
}

pub(crate) fn can_reach(st: &Session) -> bool {
    st.yt.is_authenticated() && st.online.load(Ordering::SeqCst)
}

pub(crate) fn need_catalog(st: &Session) -> CmdResult<()> {
    if !st.yt.is_authenticated() {
        return Err(AppError::new(
            ErrorKind::Auth,
            tr(
                "The YouTube Music catalog is available once the account is connected",
                "Каталог YouTube Music доступний після підключення акаунта",
                "Каталог YouTube Music доступен после подключения аккаунта",
            ),
        ));
    }
    if !st.online.load(Ordering::SeqCst) {
        return Err(AppError::new(
            ErrorKind::Network,
            tr(
                "Offline, only the library is available",
                "Немає мережі — доступна лише бібліотека",
                "Нет сети — доступна только библиотека",
            ),
        ));
    }
    Ok(())
}

pub(crate) fn yt_error(st: &Session, e: YtError) -> AppError {
    if matches!(e, YtError::Auth) {
        crate::account::auth_failed(&st.app, false);
    }
    e.into()
}

async fn tracks_of(st: &Session, ids: &[mixtape_core::TrackId]) -> CmdResult<Vec<Track>> {
    Ok(st.db.read(|c| repo::tracks_by_ids(c, ids))?)
}

/// Event: covers that landed after a search answered; the payload is their keys.
const COVERS_READY: &str = "covers:ready";

/// Where a search's thumbnails go once the answer is out: one background task fetches them
/// and announces each as it lands (`covers:ready`), so a tile draws without a new search.
fn cover_sink(st: &Session) -> CoverSink<HttpCovers> {
    let app = st.app.clone();
    CoverSink {
        db: st.db.clone(),
        store: st.covers.clone(),
        fetcher: HttpCovers::new(st.yt.http().clone(), st.covers.clone()),
        on_ready: Arc::new(move |key| {
            crate::host::emit(&app, COVERS_READY, vec![key.to_owned()]);
        }),
    }
}

fn search_error(st: &Session, e: SearchError<YtError>) -> AppError {
    match e {
        // The UI has asked again and drops this answer by its sequence number.
        SearchError::Superseded => AppError::new(ErrorKind::Internal, "search superseded"),
        SearchError::Read(e) => yt_error(st, e),
        SearchError::Store(e) => e.into(),
        SearchError::Internal(e) => AppError::new(ErrorKind::Internal, e),
    }
}

/// Reads a stored page back in the library's shapes (the likes and downloads are current).
/// The flag is false when a row is missing, e.g. cleaned away since a cached search.
async fn page_of(st: &Arc<Session>, stored: Stored) -> CmdResult<(CatalogPage, bool)> {
    let st = st.clone();
    blocking(move || {
        let Stored {
            top,
            tracks: track_ids,
            albums: album_ids,
            artists: artist_ids,
            playlists,
            continuation,
            ..
        } = stored;
        let (tracks, albums, artists, owned) = st.db.read(|c| {
            let owned: Vec<AlbumId> = album_ids
                .iter()
                .filter(|id| repo::album_owned(c, id).unwrap_or(false))
                .cloned()
                .collect();
            Ok((
                repo::tracks_by_ids(c, &track_ids)?,
                repo::albums_by_ids(c, &album_ids)?,
                repo::artists_by_ids(c, &artist_ids)?,
                owned,
            ))
        })?;
        let complete = tracks.len() == track_ids.len()
            && albums.len() == album_ids.len()
            && artists.len() == artist_ids.len();
        Ok((
            assemble(top, tracks, albums, artists, playlists, owned, continuation),
            complete,
        ))
    })
    .await
}

/// Takes the top card off the front of its list; a top card that failed to store is dropped.
fn assemble(
    top: Option<TopKind>,
    mut tracks: Vec<Track>,
    mut albums: Vec<Album>,
    mut artists: Vec<Artist>,
    mut playlists: Vec<Playlist>,
    owned_albums: Vec<AlbumId>,
    continuation: Option<String>,
) -> CatalogPage {
    let top = match top {
        Some(TopKind::Track) if !tracks.is_empty() => Some(CatalogTop::Track {
            track: tracks.remove(0),
        }),
        Some(TopKind::Album) if !albums.is_empty() => Some(CatalogTop::Album {
            album: albums.remove(0),
        }),
        Some(TopKind::Artist) if !artists.is_empty() => Some(CatalogTop::Artist {
            artist: artists.remove(0),
        }),
        Some(TopKind::Playlist) if !playlists.is_empty() => Some(CatalogTop::Playlist {
            playlist: playlists.remove(0),
        }),
        _ => None,
    };
    CatalogPage {
        top,
        tracks,
        albums,
        artists,
        playlists,
        owned_albums,
        continuation,
    }
}

/// Searches the catalog. The answer is out as soon as YouTube has answered and the rows are
/// stored; the thumbnails follow (`covers:ready`). A repeat of a query within ten minutes
/// comes from the cache, and a newer search cancels this one with its thumbnails.
#[tauri::command]
pub async fn catalog_search(
    state: St<'_>,
    query: String,
    kind: CatalogKind,
) -> CmdResult<CatalogPage> {
    let st = state.inner().clone();
    need_catalog(&st)?;
    let (query, kind) = (query.trim().to_owned(), SearchKind::from(kind));
    let key = SearchKey::new(&query, kind);
    let yt = st.yt.clone();
    let stored = st
        .search
        .search(key.clone(), cover_sink(&st), move || async move {
            yt.search(&query, kind).await
        })
        .await
        .map_err(|e| search_error(&st, e))?;
    let (page, complete) = page_of(&st, stored).await?;
    if !complete {
        st.search.forget(&key);
    }
    Ok(page)
}

#[tauri::command]
pub async fn catalog_more(
    state: St<'_>,
    kind: CatalogKind,
    continuation: String,
) -> CmdResult<CatalogPage> {
    let st = state.inner().clone();
    need_catalog(&st)?;
    let yt = st.yt.clone();
    let stored = st
        .search
        .more(cover_sink(&st), move || async move {
            yt.search_more(kind.into(), &continuation).await
        })
        .await
        .map_err(|e| search_error(&st, e))?;
    Ok(page_of(&st, stored).await?.0)
}

#[tauri::command]
pub async fn catalog_suggest(state: St<'_>, query: String) -> CmdResult<Vec<String>> {
    let st = state.inner().clone();
    if !can_reach(&st) {
        return Ok(Vec::new());
    }
    st.yt
        .suggest(query.trim())
        .await
        .map_err(|e| yt_error(&st, e))
}

/// The artist's popular tracks and full discography from YouTube Music.
#[tauri::command]
pub async fn artist_catalog(state: St<'_>, id: ArtistId) -> CmdResult<ArtistCatalog> {
    let st = state.inner().clone();
    need_catalog(&st)?;
    let found = catalog::store_artist(&*st.yt, st.yt.http(), &st.db, &st.covers, &id)
        .await
        .map_err(|e| yt_error(&st, e))?;
    let top = tracks_of(&st, &found.top).await?;
    let albums = st.db.read(|c| repo::albums_by_ids(c, &found.albums))?;
    Ok(ArtistCatalog { top, albums })
}

/// «Скачать альбом» / «Скачать плейлист» / «Скачать дискографию»; runs in the background.
#[tauri::command]
pub async fn catalog_download(state: St<'_>, kind: DownloadKind, id: String) -> CmdResult<()> {
    let st = state.inner().clone();
    need_catalog(&st)?;
    tauri::async_runtime::spawn(async move {
        let (yt, http) = (&*st.yt, st.yt.http());
        let res = match kind {
            DownloadKind::Album => {
                catalog::store_album(yt, http, &st.db, &st.covers, &AlbumId(id), true).await
            }
            DownloadKind::Artist => {
                catalog::download_artist(yt, http, &st.db, &st.covers, &ArtistId(id)).await
            }
            DownloadKind::Playlist => sources::import_link(
                yt,
                http,
                &st.db,
                &st.covers,
                url_import::Link::Playlist(PlaylistId(id)),
            )
            .await
            .map(|r| r.track_count as usize),
        };
        match res {
            Ok(n) => {
                let n = crate::events::tracks_word(n);
                crate::events::toast_info(
                    &st.app,
                    trf!(
                        "Queued for download: {n}",
                        "У завантаженнях: {n}",
                        "В загрузках: {n}"
                    ),
                );
                emit_changed(&st.app, &[ViewKind::Playlists, ViewKind::Albums]);
                st.downloads.wake();
            }
            Err(e) => {
                let e = yt_error(&st, e);
                let m = e.message;
                crate::events::toast_error(
                    &st.app,
                    trf!(
                        "Couldn't add to downloads: {m}",
                        "Не вдалося додати до завантажень: {m}",
                        "Не удалось добавить в загрузки: {m}"
                    ),
                );
            }
        }
    });
    Ok(())
}

/// Album page fallback: an album known only partly (or not at all) is read from the catalog.
pub async fn fill_album(st: &Session, id: &AlbumId, known: Option<(usize, Option<u32>)>) -> bool {
    let complete = matches!(known, Some((n, Some(total))) if n >= total as usize && n > 0);
    if complete || !can_reach(st) {
        return false;
    }
    catalog::store_album(&*st.yt, st.yt.http(), &st.db, &st.covers, id, false)
        .await
        .map_err(|e| tracing::warn!(album = %id, error = %e, "catalog album failed"))
        .is_ok()
}

/// Artist page fallback for an artist not stored yet (opened from the catalog).
pub async fn fill_artist(st: &Session, id: &ArtistId) -> bool {
    if !id.as_str().starts_with("UC") || !can_reach(st) {
        return false;
    }
    catalog::store_artist(&*st.yt, st.yt.http(), &st.db, &st.covers, id)
        .await
        .is_ok()
}

/// The first of the song's tracks whose title fits, else the top result; for a song named by
/// Shazam, whose spelling differs a little.
pub(crate) async fn first_track(
    st: &Arc<Session>,
    query: &str,
    title: &str,
) -> CmdResult<Option<Track>> {
    let yt = st.yt.clone();
    let query = query.trim().to_owned();
    let stored = st
        .search
        .lookup(cover_sink(st), move || async move {
            yt.search(&query, SearchKind::Tracks).await
        })
        .await
        .map_err(|e| search_error(st, e))?;
    let tracks = page_of(st, stored).await?.0.tracks;
    let want = title.to_lowercase();
    let pick = tracks
        .iter()
        .take(5)
        .position(|t| {
            let have = t.title.to_lowercase();
            have.contains(&want) || want.contains(&have)
        })
        .unwrap_or(0);
    Ok(tracks.into_iter().nth(pick))
}

/// A list on screen is read again after this long.
const PAGE_FRESH: Duration = Duration::from_secs(30 * 60);
/// Playlists remembered; the one read longest ago goes first.
const PAGES_KEPT: usize = 20;

type ReadPage = (Playlist, Vec<TrackId>);

/// The last read of each playlist outside the library. A generated radio answers with other
/// tracks on every read, so the list on screen and the list a click plays must be one read.
#[derive(Default)]
pub struct CatalogPages {
    pages: Mutex<Vec<(PlaylistId, ReadPage, Instant)>>,
    /// One read at a time, or a slower older answer could replace the one on screen.
    reading: tokio::sync::Mutex<()>,
}

impl CatalogPages {
    fn kept(&self) -> std::sync::MutexGuard<'_, Vec<(PlaylistId, ReadPage, Instant)>> {
        self.pages.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn get(&self, id: &PlaylistId, max_age: Duration) -> Option<ReadPage> {
        let kept = self.kept();
        let (_, page, at) = kept.iter().find(|(key, ..)| key == id)?;
        (at.elapsed() < max_age).then(|| page.clone())
    }

    fn put(&self, id: &PlaylistId, page: ReadPage) {
        let mut kept = self.kept();
        kept.retain(|(key, ..)| key != id);
        kept.push((id.clone(), page, Instant::now()));
        if kept.len() > PAGES_KEPT {
            kept.remove(0);
        }
    }

    /// The account changed: its radios are not the next one's.
    pub fn clear(&self) {
        self.kept().clear();
    }

    /// The kept page if it is younger than `max_age`, otherwise a new read.
    async fn get_or_read<E>(
        &self,
        id: &PlaylistId,
        max_age: Duration,
        read: impl Future<Output = Result<ReadPage, E>>,
    ) -> Result<ReadPage, E> {
        if let Some(page) = self.get(id, max_age) {
            return Ok(page);
        }
        let _one = self.reading.lock().await;
        // Another call may have read it while this one waited.
        if let Some(page) = self.get(id, max_age) {
            return Ok(page);
        }
        let page = read.await?;
        self.put(id, page.clone());
        Ok(page)
    }
}

async fn catalog_page(
    st: &Session,
    id: &PlaylistId,
    max_age: Duration,
) -> CmdResult<(Playlist, Vec<Track>)> {
    need_catalog(st)?;
    let read = async {
        catalog::playlist_page(&*st.yt, st.yt.http(), &st.db, &st.covers, id)
            .await
            .map_err(|e| yt_error(st, e))
    };
    let (playlist, ids) = st.catalog_pages.get_or_read(id, max_age, read).await?;
    Ok((playlist, tracks_of(st, &ids).await?))
}

/// Playlist page fallback: a playlist that is not in the library, from the catalog.
pub async fn catalog_playlist(st: &Session, id: &PlaylistId) -> CmdResult<(Playlist, Vec<Track>)> {
    catalog_page(st, id, PAGE_FRESH).await
}

/// The same playlist for playing: the list the user sees and clicks in, however old it is.
pub async fn catalog_playlist_to_play(
    st: &Session,
    id: &PlaylistId,
) -> CmdResult<(Playlist, Vec<Track>)> {
    catalog_page(st, id, Duration::MAX).await
}

#[cfg(test)]
mod assemble_tests {
    use super::*;
    use mixtape_core::PlaylistOrigin;

    fn list(id: &str) -> Playlist {
        Playlist {
            id: PlaylistId::new(id.to_owned()),
            title: id.to_owned(),
            origin: PlaylistOrigin::Ytm,
            track_count: 1,
            cover_key: None,
        }
    }

    fn page(top: Option<TopKind>, lists: Vec<Playlist>) -> CatalogPage {
        assemble(top, vec![], vec![], vec![], lists, vec![], Some("c".into()))
    }

    #[test]
    fn the_top_card_comes_off_the_front_of_its_list() {
        let p = page(Some(TopKind::Playlist), vec![list("a"), list("b")]);
        assert!(
            matches!(p.top, Some(CatalogTop::Playlist { ref playlist }) if playlist.id.as_str() == "a")
        );
        assert_eq!(p.playlists.len(), 1);
        assert_eq!(p.playlists[0].id.as_str(), "b");
        assert_eq!(p.continuation.as_deref(), Some("c"));
    }

    #[test]
    fn no_top_leaves_every_list_whole() {
        let p = page(None, vec![list("a"), list("b")]);
        assert!(p.top.is_none());
        assert_eq!(p.playlists.len(), 2);
    }

    #[test]
    fn a_top_card_that_did_not_store_is_dropped_not_taken_from_another_list() {
        // The top was a track, but no track came back: the playlists stay whole.
        let p = page(Some(TopKind::Track), vec![list("a")]);
        assert!(p.top.is_none());
        assert_eq!(p.playlists.len(), 1);
    }
}

#[cfg(test)]
mod pages_tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn id(s: &str) -> PlaylistId {
        PlaylistId::new(s.to_owned())
    }

    fn page(n: u32) -> ReadPage {
        let playlist = Playlist {
            id: id("VLRDATradio"),
            title: format!("read {n}"),
            origin: mixtape_core::PlaylistOrigin::Ytm,
            track_count: 1,
            cover_key: None,
        };
        (
            playlist,
            vec![TrackId::from(format!("track{n:06}").as_str())],
        )
    }

    /// Answers like a generated radio: other tracks on every read.
    async fn radio(reads: &AtomicU32) -> Result<ReadPage, ()> {
        Ok(page(reads.fetch_add(1, Ordering::SeqCst)))
    }

    #[tokio::test]
    async fn the_list_clicked_is_the_list_played() {
        let (pages, reads) = (CatalogPages::default(), AtomicU32::new(0));
        let list = id("VLRDATradio");
        let shown = pages
            .get_or_read(&list, PAGE_FRESH, radio(&reads))
            .await
            .unwrap();
        let played = pages
            .get_or_read(&list, Duration::MAX, radio(&reads))
            .await
            .unwrap();
        let shown_again = pages
            .get_or_read(&list, PAGE_FRESH, radio(&reads))
            .await
            .unwrap();
        assert_eq!(shown.1, played.1);
        assert_eq!(
            shown.1, shown_again.1,
            "library:changed must not replace the list on screen"
        );
        assert_eq!(reads.load(Ordering::SeqCst), 1, "a click costs no read");
    }

    #[tokio::test]
    async fn an_old_list_is_read_again_only_for_the_screen() {
        let (pages, reads) = (CatalogPages::default(), AtomicU32::new(0));
        let list = id("VLRDATradio");
        let first = pages
            .get_or_read(&list, PAGE_FRESH, radio(&reads))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let played = pages
            .get_or_read(&list, Duration::MAX, radio(&reads))
            .await
            .unwrap();
        assert_eq!(first.1, played.1, "a click plays the old list on screen");
        let reopened = pages
            .get_or_read(&list, Duration::from_millis(1), radio(&reads))
            .await
            .unwrap();
        assert_ne!(first.1, reopened.1);
        let played = pages
            .get_or_read(&list, Duration::MAX, radio(&reads))
            .await
            .unwrap();
        assert_eq!(reopened.1, played.1, "and after a new read, the new one");
    }

    #[tokio::test]
    async fn a_failed_read_keeps_nothing() {
        let pages = CatalogPages::default();
        let list = id("VLRDATradio");
        let failed = pages
            .get_or_read(&list, PAGE_FRESH, async { Err::<ReadPage, ()>(()) })
            .await;
        assert!(failed.is_err());
        assert!(pages.get(&list, Duration::MAX).is_none());
    }

    #[test]
    fn keeps_a_limited_number_of_lists() {
        let pages = CatalogPages::default();
        for n in 0..=PAGES_KEPT as u32 {
            pages.put(&id(&format!("VLRD{n}")), page(n));
        }
        assert!(
            pages.get(&id("VLRD0"), Duration::MAX).is_none(),
            "the oldest goes"
        );
        assert!(pages.get(&id("VLRD1"), Duration::MAX).is_some());
        pages.clear();
        assert!(pages.get(&id("VLRD1"), Duration::MAX).is_none());
    }
}

/// Every song of an artist, owned or streamable. `load` reads all albums of the
/// discography from YouTube Music first (metadata only, nothing is downloaded); without
/// it the songs come only if they were loaded before.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArtistSongs {
    pub tracks: Vec<Track>,
    /// Unix ms of the last full load; absent = never loaded.
    #[ts(optional, type = "number")]
    pub loaded_at: Option<i64>,
}

/// `artist:loading` — progress of «Загрузить все песни».
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistLoading {
    id: String,
    done: usize,
    total: usize,
}

fn songs_key(id: &ArtistId) -> String {
    format!("artist_songs_loaded:{id}")
}

#[tauri::command]
pub async fn artist_songs(
    state: St<'_>,
    id: ArtistId,
    load: bool,
) -> CmdResult<Option<ArtistSongs>> {
    let st = state.inner().clone();
    let key = songs_key(&id);
    let mut loaded_at: Option<i64> = mixtape_library::settings::get(&st.db, &key)?;
    if load {
        need_catalog(&st)?;
        let artist = st.yt.artist(&id).await.map_err(|e| yt_error(&st, e))?;
        let albums: Vec<AlbumId> = artist.discography.iter().map(|c| c.id.clone()).collect();
        catalog::store_album_cards(st.yt.http(), &st.db, &st.covers, &artist.discography).await;
        let total = albums.len();
        let sem = Arc::new(tokio::sync::Semaphore::new(3));
        let mut set = tokio::task::JoinSet::new();
        for album in albums {
            let (st, sem) = (st.clone(), sem.clone());
            set.spawn(async move {
                let _permit = sem.acquire().await.ok()?;
                // Albums already read in full are not fetched again.
                let (n, count) = st
                    .db
                    .read(|c| {
                        Ok((
                            repo::album_track_ids(c, &album)?.len(),
                            repo::album(c, &album)?.and_then(|a| a.track_count),
                        ))
                    })
                    .ok()?;
                if count.is_some_and(|c| c > 0 && n >= c as usize) {
                    return Some(());
                }
                catalog::store_album(&*st.yt, st.yt.http(), &st.db, &st.covers, &album, false)
                    .await
                    .ok()
                    .map(drop)
            });
        }
        let mut done = 0;
        while set.join_next().await.is_some() {
            done += 1;
            crate::host::emit(
                &st.app,
                "artist:loading",
                ArtistLoading {
                    id: id.to_string(),
                    done,
                    total,
                },
            );
        }
        let now = repo::now_ms();
        mixtape_library::settings::set(&st.db, &key, &now)?;
        loaded_at = Some(now);
    }
    let Some(loaded_at) = loaded_at else {
        return Ok(None);
    };
    let tracks = st
        .db
        .read(|c| repo::tracks_by_ids(c, &repo::artist_all_track_ids(c, &id)?))?;
    Ok(Some(ArtistSongs {
        tracks,
        loaded_at: Some(loaded_at),
    }))
}
