//! The YouTube Music catalog (US4): search outside the library, artist pages from the
//! catalog and «Скачать» for albums, playlists and discographies.

use super::CmdResult;
use crate::commands::library::emit_changed;
use crate::state::AppState;
use mixtape_core::{Album, AlbumId, AppError, Artist, ArtistId, ErrorKind, Playlist, PlaylistId, Track, tr, trf};
use mixtape_library::repo;
use mixtape_library::views::ViewKind;
use mixtape_sync::{catalog, sources, url_import};
use mixtape_yt::YtError;
use mixtape_yt::api::YtMusic;
use mixtape_yt::search::{SearchItem, SearchKind, SearchPage};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tauri::{Emitter, State};
use ts_rs::TS;

type St<'a> = State<'a, Arc<AppState>>;

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

pub(crate) fn can_reach(st: &AppState) -> bool {
    st.yt.is_authenticated() && st.online.load(Ordering::SeqCst)
}

fn need_catalog(st: &AppState) -> CmdResult<()> {
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
            tr("Offline, only the library is available", "Немає мережі — доступна лише бібліотека", "Нет сети — доступна только библиотека"),
        ));
    }
    Ok(())
}

fn yt_error(st: &AppState, e: YtError) -> AppError {
    if matches!(e, YtError::Auth) {
        let _ = st.app.emit("account:expired", ());
    }
    e.into()
}

async fn tracks_of(st: &AppState, ids: &[mixtape_core::TrackId]) -> CmdResult<Vec<Track>> {
    Ok(st.db.read(|c| repo::tracks_by_ids(c, ids))?)
}

/// Stores what the page found and reads it back in the library's shapes.
async fn page_of(st: &AppState, page: SearchPage) -> CmdResult<CatalogPage> {
    let http = st.yt.http();
    let mut tracks = page.tracks;
    let mut albums = page.albums;
    let mut artists = page.artists;
    let mut playlists = page.playlists;
    // The top card is stored with the rest, then picked back out.
    let top_kind = page.top.map(|top| match top {
        SearchItem::Track(t) => {
            tracks.insert(0, t);
            0
        }
        SearchItem::Album(a) => {
            albums.insert(0, a);
            1
        }
        SearchItem::Artist(a) => {
            artists.insert(0, a);
            2
        }
        SearchItem::Playlist(p) => {
            playlists.insert(0, p);
            3
        }
    });
    let track_ids = sources::store_tracks(http, &st.db, &st.covers, tracks).await?;
    let album_ids = catalog::store_album_cards(http, &st.db, &st.covers, &albums).await;
    let artist_ids = catalog::store_artist_cards(http, &st.db, &st.covers, &artists).await;
    let mut playlists = catalog::playlist_cards(http, &st.covers, playlists).await;
    let (mut tracks, mut albums, mut artists, owned) = st.db.read(|c| {
        let owned: Vec<AlbumId> = album_ids.iter().filter(|id| repo::album_owned(c, id).unwrap_or(false)).cloned().collect();
        Ok((
            repo::tracks_by_ids(c, &track_ids)?,
            repo::albums_by_ids(c, &album_ids)?,
            repo::artists_by_ids(c, &artist_ids)?,
            owned,
        ))
    })?;
    let top = match top_kind {
        Some(0) if !tracks.is_empty() => Some(CatalogTop::Track { track: tracks.remove(0) }),
        Some(1) if !albums.is_empty() => Some(CatalogTop::Album { album: albums.remove(0) }),
        Some(2) if !artists.is_empty() => Some(CatalogTop::Artist { artist: artists.remove(0) }),
        Some(3) if !playlists.is_empty() => Some(CatalogTop::Playlist { playlist: playlists.remove(0) }),
        _ => None,
    };
    Ok(CatalogPage { top, tracks, albums, artists, playlists, owned_albums: owned, continuation: page.continuation })
}

#[tauri::command]
pub async fn catalog_search(state: St<'_>, query: String, kind: CatalogKind) -> CmdResult<CatalogPage> {
    let st = state.inner().clone();
    need_catalog(&st)?;
    let page = st.yt.search(query.trim(), kind.into()).await.map_err(|e| yt_error(&st, e))?;
    page_of(&st, page).await
}

#[tauri::command]
pub async fn catalog_more(state: St<'_>, kind: CatalogKind, continuation: String) -> CmdResult<CatalogPage> {
    let st = state.inner().clone();
    need_catalog(&st)?;
    let page = st.yt.search_more(kind.into(), &continuation).await.map_err(|e| yt_error(&st, e))?;
    page_of(&st, page).await
}

#[tauri::command]
pub async fn catalog_suggest(state: St<'_>, query: String) -> CmdResult<Vec<String>> {
    let st = state.inner().clone();
    if !can_reach(&st) {
        return Ok(Vec::new());
    }
    st.yt.suggest(query.trim()).await.map_err(|e| yt_error(&st, e))
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
            DownloadKind::Album => catalog::store_album(yt, http, &st.db, &st.covers, &AlbumId(id), true).await,
            DownloadKind::Artist => catalog::download_artist(yt, http, &st.db, &st.covers, &ArtistId(id)).await,
            DownloadKind::Playlist => {
                sources::import_link(yt, http, &st.db, &st.covers, url_import::Link::Playlist(PlaylistId(id)))
                    .await
                    .map(|r| r.track_count as usize)
            }
        };
        match res {
            Ok(n) => {
                let n = crate::events::tracks_word(n);
                crate::events::toast_info(&st.app, trf!("Queued for download: {n}", "У завантаженнях: {n}", "В загрузках: {n}"));
                emit_changed(&st.app, &[ViewKind::Playlists, ViewKind::Albums]);
                st.downloads.wake();
            }
            Err(e) => {
                let e = yt_error(&st, e);
                let m = e.message;
                crate::events::toast_error(
                    &st.app,
                    trf!("Couldn't add to downloads: {m}", "Не вдалося додати до завантажень: {m}", "Не удалось добавить в загрузки: {m}"),
                );
            }
        }
    });
    Ok(())
}

/// Album page fallback: an album known only partly (or not at all) is read from the catalog.
pub async fn fill_album(st: &AppState, id: &AlbumId, known: Option<(usize, Option<u32>)>) -> bool {
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
pub async fn fill_artist(st: &AppState, id: &ArtistId) -> bool {
    if !id.as_str().starts_with("UC") || !can_reach(st) {
        return false;
    }
    catalog::store_artist(&*st.yt, st.yt.http(), &st.db, &st.covers, id).await.is_ok()
}

/// Playlist page fallback: a playlist that is not in the library, straight from the catalog.
pub async fn catalog_playlist(st: &AppState, id: &PlaylistId) -> CmdResult<(Playlist, Vec<Track>)> {
    need_catalog(st)?;
    let (playlist, ids) = catalog::playlist_page(&*st.yt, st.yt.http(), &st.db, &st.covers, id)
        .await
        .map_err(|e| yt_error(st, e))?;
    Ok((playlist, tracks_of(st, &ids).await?))
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
pub async fn artist_songs(state: St<'_>, id: ArtistId, load: bool) -> CmdResult<Option<ArtistSongs>> {
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
                    .read(|c| Ok((repo::album_track_ids(c, &album)?.len(), repo::album(c, &album)?.and_then(|a| a.track_count))))
                    .ok()?;
                if count.is_some_and(|c| c > 0 && n >= c as usize) {
                    return Some(());
                }
                catalog::store_album(&*st.yt, st.yt.http(), &st.db, &st.covers, &album, false).await.ok().map(drop)
            });
        }
        let mut done = 0;
        while set.join_next().await.is_some() {
            done += 1;
            let _ = st.app.emit("artist:loading", ArtistLoading { id: id.to_string(), done, total });
        }
        let now = repo::now_ms();
        mixtape_library::settings::set(&st.db, &key, &now)?;
        loaded_at = Some(now);
    }
    let Some(loaded_at) = loaded_at else { return Ok(None) };
    let tracks = st.db.read(|c| repo::tracks_by_ids(c, &repo::artist_all_track_ids(c, &id)?))?;
    Ok(Some(ArtistSongs { tracks, loaded_at: Some(loaded_at) }))
}
