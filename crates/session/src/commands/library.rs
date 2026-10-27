//! Library commands (contracts/ipc.md, «Библиотека»).

use super::{CmdResult, blocking};
use crate::state::Session;
use mixtape_core::{
    Album, AlbumId, AppError, Artist, ArtistId, ErrorKind, Playlist, PlaylistId, Track, TrackId, tr,
};
use mixtape_library::views::{self, ViewIds, ViewKind, ViewSort};
use mixtape_library::{playlists, repo, search};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};
use ts_rs::TS;

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ViewInfo {
    #[ts(type = "number")]
    pub view_id: u64,
    pub count: u32,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind", content = "items")]
#[ts(export)]
pub enum Rows {
    Tracks(Vec<Track>),
    Albums(Vec<Album>),
    Artists(Vec<Artist>),
    Playlists(Vec<Playlist>),
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchResult {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlbumPage {
    pub album: Album,
    pub tracks: Vec<Track>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArtistPage {
    pub artist: Artist,
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlaylistPage {
    pub playlist: Playlist,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LibraryChanged {
    pub kinds: Vec<ViewKind>,
}

pub fn emit_changed(app: &AppHandle, kinds: &[ViewKind]) {
    crate::host::emit(app, "library:changed", LibraryChanged { kinds: kinds.to_vec() });
}

fn not_found(message: &str) -> AppError {
    AppError::new(ErrorKind::NotFound, message)
}

#[tauri::command]
pub async fn library_view(
    state: State<'_, Arc<Session>>,
    kind: ViewKind,
    sort: Option<ViewSort>,
    filter: Option<String>,
) -> CmdResult<ViewInfo> {
    let st = state.inner().clone();
    blocking(move || {
        let filter = filter.unwrap_or_default();
        let ids = st.db.read(|c| views::build_filtered(c, kind, sort.unwrap_or_default(), &filter))?;
        let count = ids.len() as u32;
        Ok(ViewInfo { view_id: st.views.insert(ids), count })
    })
    .await
}

#[tauri::command]
pub async fn library_rows(
    state: State<'_, Arc<Session>>,
    view_id: u64,
    offset: u32,
    limit: u32,
) -> CmdResult<Rows> {
    let st = state.inner().clone();
    blocking(move || {
        let slice = st
            .views
            .slice(view_id, offset as usize, limit.min(200) as usize)
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, tr("The list is out of date, refresh the screen", "Список застарів, оновіть екран", "Список устарел, обновите экран")))?;
        Ok(st.db.read(|c| {
            Ok(match slice {
                ViewIds::Tracks(ids) => Rows::Tracks(repo::tracks_by_ids(c, &ids)?),
                ViewIds::Albums(ids) => Rows::Albums(repo::albums_by_ids(c, &ids)?),
                ViewIds::Artists(ids) => Rows::Artists(repo::artists_by_ids(c, &ids)?),
                ViewIds::Playlists(ids) => Rows::Playlists(repo::playlists_by_ids(c, &ids)?),
            })
        })?)
    })
    .await
}

#[tauri::command]
pub async fn library_search(
    state: State<'_, Arc<Session>>,
    query: String,
    limit: Option<u32>,
) -> CmdResult<SearchResult> {
    let st = state.inner().clone();
    blocking(move || {
        Ok(st.db.read(|c| {
            let ids = search::search(c, &query, limit.unwrap_or(30) as usize)?;
            Ok(SearchResult {
                tracks: repo::tracks_by_ids(c, &ids.tracks)?,
                albums: repo::albums_by_ids(c, &ids.albums)?,
                artists: repo::artists_by_ids(c, &ids.artists)?,
                playlists: repo::playlists_by_ids(c, &ids.playlists)?,
            })
        })?)
    })
    .await
}

/// One track as the library has it now (the player refreshes its copy after likes and
/// downloads).
#[tauri::command]
pub async fn track_get(state: State<'_, Arc<Session>>, id: TrackId) -> CmdResult<Option<Track>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.db.read(|c| repo::track(c, &id))?)).await
}

#[tauri::command]
pub async fn album_get(state: State<'_, Arc<Session>>, id: AlbumId) -> CmdResult<AlbumPage> {
    let st = state.inner().clone();
    let page = album_page(st.clone(), id.clone()).await?;
    // Known only in part (a liked single, a search card) or not at all: read the catalog.
    let known = page.as_ref().map(|p| (p.tracks.len(), p.album.track_count));
    if crate::commands::catalog::fill_album(&st, &id, known).await {
        return album_page(st, id).await?.ok_or_else(|| not_found(tr("Album not found", "Альбом не знайдено", "Альбом не найден")));
    }
    page.ok_or_else(|| not_found(tr("Album not found", "Альбом не знайдено", "Альбом не найден")))
}

async fn album_page(st: Arc<Session>, id: AlbumId) -> CmdResult<Option<AlbumPage>> {
    blocking(move || {
        Ok(st.db.read(|c| {
            let Some(album) = repo::album(c, &id)? else { return Ok(None) };
            let tracks = repo::tracks_by_ids(c, &repo::album_track_ids(c, &id)?)?;
            Ok(Some(AlbumPage { album, tracks }))
        })?)
    })
    .await
}

#[tauri::command]
pub async fn artist_get(state: State<'_, Arc<Session>>, id: ArtistId) -> CmdResult<ArtistPage> {
    let st = state.inner().clone();
    let mut page = match artist_page(st.clone(), id.clone()).await {
        Err(e) if e.kind == ErrorKind::NotFound && crate::commands::catalog::fill_artist(&st, &id).await => {
            artist_page(st.clone(), id.clone()).await?
        }
        r => r?,
    };
    // Pictures fetched before palettes were stored: compute it once from the thumbnail.
    if let Some(key) = page.artist.image_key.clone()
        && page.artist.palette.is_empty()
        && let Ok(palette) = st.covers.palette(&key)
    {
        let (aid, k, p) = (id.clone(), key.clone(), palette.clone());
        let _ = st.db.write(move |t| mixtape_library::ytm::set_artist_image(t, &aid, &k, &p));
        page.artist.palette = palette;
    }
    // No picture yet: fetch it in the background (not again within a week if the artist
    // has none); the page reloads on `library:changed`.
    if page.artist.image_key.is_none()
        && id.as_str().starts_with("UC")
        && st.yt.is_authenticated()
        && mixtape_sync::artists::image_due(&st.db, &id)
    {
        tauri::async_runtime::spawn(async move {
            let in_library = st.db.read(|c| mixtape_library::ytm::artist_in_library(c, &id)).unwrap_or(false);
            let detail = if in_library { mixtape_artwork::Detail::Full } else { mixtape_artwork::Detail::Small };
            if mixtape_sync::artists::fetch_image(&*st.yt, st.yt.http(), &st.db, &st.covers, &id, detail).await {
                emit_changed(&st.app, &[ViewKind::Artists]);
            }
        });
    }
    Ok(page)
}

async fn artist_page(st: Arc<Session>, id: ArtistId) -> CmdResult<ArtistPage> {
    blocking(move || {
        let page = st.db.read(|c| {
            let Some(artist) = repo::artist(c, &id)? else { return Ok(None) };
            let tracks = repo::tracks_by_ids(c, &repo::artist_track_ids(c, &id)?)?;
            let albums = repo::albums_by_ids(c, &repo::artist_album_ids(c, &id)?)?;
            Ok(Some(ArtistPage { artist, tracks, albums }))
        })?;
        page.ok_or_else(|| not_found(tr("Artist not found", "Виконавця не знайдено", "Исполнитель не найден")))
    })
    .await
}

#[tauri::command]
pub async fn playlist_get(state: State<'_, Arc<Session>>, id: PlaylistId) -> CmdResult<PlaylistPage> {
    let st = state.inner().clone();
    let st2 = st.clone();
    let id2 = id.clone();
    let page = blocking(move || {
        Ok(st2.db.read(|c| {
            let Some(playlist) = repo::playlist(c, &id2)? else { return Ok(None) };
            let tracks = repo::tracks_by_ids(c, &repo::playlist_track_ids(c, &id2)?)?;
            Ok(Some(PlaylistPage { playlist, tracks }))
        })?)
    })
    .await?;
    match page {
        Some(p) => Ok(p),
        // Not in the library: show it from the catalog without adding it.
        None if id.as_str().starts_with("VL") || id.as_str().starts_with("PL") => {
            let (playlist, tracks) = crate::commands::catalog::catalog_playlist(&st, &id).await?;
            Ok(PlaylistPage { playlist, tracks })
        }
        None => Err(not_found(tr("Playlist not found", "Плейлист не знайдено", "Плейлист не найден"))),
    }
}

/// Playlist edits. Each returns the id that was edited: editing a YouTube Music mirror
/// creates a local copy, and the UI should switch to it.
#[derive(serde::Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "op")]
#[ts(export)]
pub enum PlaylistEdit {
    Create { title: String },
    Rename { id: PlaylistId, title: String },
    Delete { id: PlaylistId },
    Add { id: PlaylistId, track_ids: Vec<TrackId> },
    Remove { id: PlaylistId, position: u32 },
    Move { id: PlaylistId, from: u32, to: u32 },
}

#[tauri::command]
pub async fn playlist_edit(
    app: AppHandle,
    state: State<'_, Arc<Session>>,
    edit: PlaylistEdit,
) -> CmdResult<Option<PlaylistId>> {
    let st = state.inner().clone();
    let res = blocking(move || {
        Ok(st.db.write(move |t| {
            Ok(match edit {
                PlaylistEdit::Create { title } => Some(playlists::create(t, &title)?),
                PlaylistEdit::Rename { id, title } => Some(playlists::rename(t, &id, &title)?),
                PlaylistEdit::Delete { id } => {
                    playlists::delete(t, &id)?;
                    None
                }
                PlaylistEdit::Add { id, track_ids } => Some(playlists::add(t, &id, &track_ids)?),
                PlaylistEdit::Remove { id, position } => Some(playlists::remove(t, &id, position)?),
                PlaylistEdit::Move { id, from, to } => Some(playlists::move_item(t, &id, from, to)?),
            })
        })?)
    })
    .await?;
    emit_changed(&app, &[ViewKind::Playlists]);
    Ok(res)
}
