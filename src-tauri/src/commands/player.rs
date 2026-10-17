//! Playback and queue commands (contracts/ipc.md, «Воспроизведение и очередь»).

use super::{CmdResult, blocking};
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind, TrackId, tr};
use mixtape_library::repo;
use mixtape_player::{PlaySource, PlaybackState, QueueContext, QueueView, Repeat};
use serde::Deserialize;
use std::sync::Arc;
use tauri::State;
use tauri::ipc::Channel;
use ts_rs::TS;

type St<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
pub async fn player_subscribe(state: St<'_>, channel: Channel<PlaybackState>) -> CmdResult<()> {
    let _ = channel.send(state.player.state());
    *state.player_channel.lock().expect("channel slot poisoned") = Some(channel);
    // Re-announce the current track so a reloaded UI shows it right away.
    let st = state.inner().clone();
    blocking(move || {
        if let Some(track) = st.player.current_track() {
            crate::events::announce_track(&st, track);
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn play(state: St<'_>, source: PlaySource, start_index: Option<u32>) -> CmdResult<()> {
    let st = state.inner().clone();
    // Catalog items (search, home) may not have their tracks stored yet.
    let source = match source {
        PlaySource::Album { id } => {
            let (n, total) = st.db.read(|c| {
                Ok((repo::album_track_ids(c, &id)?.len(), repo::album(c, &id)?.and_then(|a| a.track_count)))
            })?;
            crate::commands::catalog::fill_album(&st, &id, Some((n, total))).await;
            PlaySource::Album { id }
        }
        PlaySource::Playlist { id } if st.db.read(|c| repo::playlist(c, &id))?.is_none() => {
            let (_, tracks) = crate::commands::catalog::catalog_playlist(&st, &id).await?;
            let ids = tracks.into_iter().map(|t| t.id).collect();
            return blocking(move || {
                st.player.play_tracks(ids, start_index.unwrap_or(0) as usize, QueueContext::Playlist(id));
                Ok(())
            })
            .await;
        }
        PlaySource::Artist { id } if st.db.read(|c| repo::artist_track_ids(c, &id))?.is_empty() => {
            let found = mixtape_sync::catalog::store_artist(&*st.yt, st.yt.http(), &st.db, &st.covers, &id)
                .await
                .map_err(AppError::from)?;
            return blocking(move || {
                st.player.play_tracks(found.top, start_index.unwrap_or(0) as usize, QueueContext::Artist(id));
                Ok(())
            })
            .await;
        }
        other => other,
    };
    blocking(move || {
        let (ids, context) = match source {
            PlaySource::Album { id } => (st.db.read(|c| repo::album_track_ids(c, &id))?, QueueContext::Album(id)),
            PlaySource::Playlist { id } => {
                (st.db.read(|c| repo::playlist_track_ids(c, &id))?, QueueContext::Playlist(id))
            }
            PlaySource::Artist { id } => (st.db.read(|c| repo::artist_track_ids(c, &id))?, QueueContext::Artist(id)),
            PlaySource::View { view_id, liked } => {
                let ids = st
                    .views
                    .track_ids(view_id)
                    .ok_or_else(|| AppError::new(ErrorKind::Invalid, tr("The list is out of date, refresh the screen", "Список застарів, оновіть екран", "Список устарел, обновите экран")))?;
                (ids, if liked { QueueContext::Liked } else { QueueContext::Library })
            }
            PlaySource::Tracks { track_ids } => (track_ids, QueueContext::Search),
        };
        st.player.play_tracks(ids, start_index.unwrap_or(0) as usize, context);
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn player_toggle(state: St<'_>) {
    state.player.toggle();
}
#[tauri::command]
pub async fn player_next(state: St<'_>) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.player.next();
        Ok(())
    })
    .await
}
#[tauri::command]
pub async fn player_prev(state: St<'_>) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.player.prev();
        Ok(())
    })
    .await
}
#[tauri::command]
pub fn player_seek(state: St<'_>, position_ms: u64) {
    state.player.seek(position_ms);
    state.media.timeline_dirty();
}
#[tauri::command]
pub fn player_set_volume(state: St<'_>, volume: f32) {
    state.player.set_volume(volume);
}
#[tauri::command]
pub fn player_set_repeat(state: St<'_>, repeat: Repeat) {
    state.player.set_repeat(repeat);
}
#[tauri::command]
pub fn player_set_shuffle(state: St<'_>, on: bool) {
    state.player.set_shuffle(on);
}

#[tauri::command]
pub async fn queue_get(state: St<'_>) -> CmdResult<QueueView> {
    let st = state.inner().clone();
    blocking(move || Ok(st.player.queue_view())).await
}

#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum QueuePosition {
    Next,
    End,
}

#[tauri::command]
pub async fn queue_add(state: St<'_>, track_ids: Vec<TrackId>, position: QueuePosition) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.player.edit_queue(|q| match position {
            QueuePosition::Next => q.play_next(track_ids),
            QueuePosition::End => q.add_to_queue(track_ids),
        });
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn queue_remove(state: St<'_>, uid: u64) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.player.edit_queue(|q| q.remove(uid));
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn queue_move(state: St<'_>, uid: u64, to_index: u32) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.player.edit_queue(|q| q.move_item(uid, to_index as usize));
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn queue_clear(state: St<'_>) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.player.edit_queue(|q| q.clear_upcoming());
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn queue_jump(state: St<'_>, uid: u64) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.player.jump(uid);
        Ok(())
    })
    .await
}
