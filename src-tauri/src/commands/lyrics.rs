//! Lyrics for the full-screen player (US5).

use super::CmdResult;
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind, TrackId};
use mixtape_library::repo;
use mixtape_sync::lyrics::{self, Lyrics};
use std::sync::Arc;
use tauri::State;

/// Lyrics of a library track; `null` when lrclib has none.
#[tauri::command]
pub async fn lyrics_get(
    state: State<'_, Arc<AppState>>,
    track_id: TrackId,
) -> CmdResult<Option<Lyrics>> {
    let st = state.inner().clone();
    let track = st
        .db
        .read(|c| repo::track(c, &track_id))?
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, "Трек не найден"))?;
    Ok(lyrics::get(&st.lrclib, &st.db, &track).await?)
}
