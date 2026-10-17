//! Spotify import: a public playlist or album link, or a pasted «Artist — Title» list,
//! becomes a local playlist of YouTube Music songs (`mixtape_sync::spotify`). One import
//! runs at a time; `spotify:progress` {done, total} follows the matching.

use super::{CmdResult, blocking};
use crate::commands::catalog::{need_catalog, yt_error};
use crate::commands::library::emit_changed;
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind, PlaylistId, TrackState, tr};
use mixtape_library::views::ViewKind;
use mixtape_library::{cache_lru, jobs, repo};
use mixtape_sync::spotify::import::{self, MatchError, SpotifyCut, SpotifyImportResult};
use mixtape_sync::spotify::{self, page};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};
use ts_rs::TS;

type St<'a> = State<'a, Arc<AppState>>;

/// What to import.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum SpotifySource {
    /// `open.spotify.com/playlist/…` or `/album/…`.
    Link { url: String },
    /// One «Artist — Title» per line; `name` is the new playlist's title.
    Lines {
        text: String,
        #[ts(optional)]
        name: Option<String>,
    },
}

/// `spotify:progress`.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpotifyProgress {
    pub done: u32,
    pub total: u32,
}

/// The running import's cancel flag.
static RUNNING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

fn running() -> std::sync::MutexGuard<'static, Option<Arc<AtomicBool>>> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Clears [`RUNNING`] however the import ends.
struct Slot;

impl Drop for Slot {
    fn drop(&mut self) {
        *running() = None;
    }
}

fn cancelled() -> AppError {
    AppError::new(
        ErrorKind::Invalid,
        tr("Import cancelled", "Імпорт скасовано", "Импорт отменён"),
    )
}

fn invalid(msg: &'static str) -> AppError {
    AppError::new(ErrorKind::Invalid, msg)
}

#[tauri::command]
pub async fn spotify_import(
    state: St<'_>,
    source: SpotifySource,
) -> CmdResult<SpotifyImportResult> {
    let st = state.inner().clone();
    need_catalog(&st)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = running();
        if slot.is_some() {
            return Err(invalid(tr(
                "Another import is still running",
                "Інший імпорт ще триває",
                "Другой импорт ещё идёт",
            )));
        }
        *slot = Some(cancel.clone());
    }
    let _slot = Slot;

    let (title, tracks, cut) = match source {
        SpotifySource::Link { url } => {
            let link = spotify::parse_link(&url).ok_or_else(|| {
                invalid(tr(
                    "This isn't a link to a Spotify playlist or album",
                    "Це не посилання на плейлист чи альбом Spotify",
                    "Это не ссылка на плейлист или альбом Spotify",
                ))
            })?;
            let list = page::fetch(st.yt.http(), &link).await?;
            let cut = list.is_cut().then_some(SpotifyCut {
                read: list.tracks.len() as u32,
                total: list.total,
            });
            let title = if list.name.is_empty() {
                tr(
                    "Spotify playlist",
                    "Плейлист зі Spotify",
                    "Плейлист из Spotify",
                )
                .to_owned()
            } else {
                list.name
            };
            (title, list.tracks, cut)
        }
        SpotifySource::Lines { text, name } => {
            let title = name
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    tr(
                        "Imported playlist",
                        "Імпортований плейлист",
                        "Импортированный плейлист",
                    )
                    .to_owned()
                });
            (title, spotify::parse_lines(&text), None)
        }
    };
    if tracks.is_empty() {
        return Err(invalid(tr(
            "No tracks to look for",
            "Немає треків для пошуку",
            "Нет треков для поиска",
        )));
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(cancelled());
    }

    let total = tracks.len() as u32;
    let app = st.app.clone();
    let _ = app.emit("spotify:progress", SpotifyProgress { done: 0, total });
    let found = import::match_all(
        st.yt.clone(),
        &tracks,
        Arc::new(import::pacer()),
        cancel.clone(),
        |done, _| {
            let _ = app.emit(
                "spotify:progress",
                SpotifyProgress {
                    done: done as u32,
                    total,
                },
            );
        },
    )
    .await
    .map_err(|e| match e {
        MatchError::Cancelled => cancelled(),
        MatchError::Yt(e) => yt_error(&st, e),
    })?;
    if cancel.load(Ordering::SeqCst) {
        return Err(cancelled());
    }
    let id = import::save_playlist(st.yt.http(), &st.db, &st.covers, &title, &found).await?;
    if id.is_some() {
        emit_changed(&st.app, &[ViewKind::Playlists, ViewKind::Tracks]);
    }
    let result = import::summary(&title, &tracks, &found, id, cut);
    tracing::info!(matched = result.matched, total = result.total, cut = ?result.cut, "spotify import done");
    Ok(result)
}

/// Stops the running import; nothing is saved.
#[tauri::command]
pub fn spotify_cancel() {
    if let Some(flag) = running().as_ref() {
        flag.store(true, Ordering::SeqCst);
    }
}

/// «Download playlist» for the imported playlist: cached songs move into the library at
/// once, the rest is queued. Returns how many tracks were queued or moved.
#[tauri::command]
pub async fn spotify_download(state: St<'_>, id: PlaylistId) -> CmdResult<u32> {
    let st = state.inner().clone();
    let st2 = st.clone();
    let (queued, promoted) = blocking(move || {
        let root = st2.paths.root.clone();
        Ok(st2.db.write(move |t| {
            let (mut queued, mut promoted) = (0u32, 0u32);
            for track in repo::playlist_track_ids(t, &id)? {
                if cache_lru::promote(t, &root, &track)? {
                    promoted += 1;
                } else if !matches!(
                    repo::track(t, &track)?.map(|x| x.state),
                    Some(TrackState::Downloaded)
                ) && jobs::enqueue(t, &track, "playlist")?.is_some()
                {
                    queued += 1;
                }
            }
            Ok((queued, promoted))
        })?)
    })
    .await?;
    if promoted > 0 {
        emit_changed(
            &st.app,
            &[ViewKind::Tracks, ViewKind::Albums, ViewKind::Artists],
        );
    }
    if queued > 0 {
        st.downloads.wake();
    }
    Ok(queued + promoted)
}
