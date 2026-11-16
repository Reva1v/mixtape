//! Recent searches under the search box. They live in the database (`mixtape_library::recent`)
//! so the phone sees the same list after an exchange; the interface keeps a copy and refreshes
//! it on `recent:changed`, which the phone sync emits when the other device changed the list.

use super::{CmdResult, blocking};
use crate::state::Session;
use mixtape_library::recent::{self, RecentItem};
use mixtape_library::repo::now_ms;
use std::sync::Arc;
use tauri::State;

/// The list, newest first.
#[tauri::command]
pub async fn recent_searches(state: State<'_, Arc<Session>>) -> CmdResult<Vec<RecentItem>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.db.read(recent::list)?)).await
}

/// Lists `item` as just used (at the top); returns the new list.
#[tauri::command]
pub async fn recent_add(
    state: State<'_, Arc<Session>>,
    item: RecentItem,
) -> CmdResult<Vec<RecentItem>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.db.write(move |t| recent::add(t, item, now_ms()))?)).await
}

/// Removes `item`; the removal reaches the phone with the next exchange.
#[tauri::command]
pub async fn recent_remove(
    state: State<'_, Arc<Session>>,
    item: RecentItem,
) -> CmdResult<Vec<RecentItem>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.db.write(move |t| recent::remove(t, &item, now_ms()))?)).await
}

/// Removes everything.
#[tauri::command]
pub async fn recent_clear(state: State<'_, Arc<Session>>) -> CmdResult<Vec<RecentItem>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.db.write(|t| recent::clear(t, now_ms()))?)).await
}

/// One-time move of the list the interface kept in the browser's storage (newest first):
/// adds what isn't listed yet, never what was removed since.
#[tauri::command]
pub async fn recent_import(
    state: State<'_, Arc<Session>>,
    items: Vec<RecentItem>,
) -> CmdResult<Vec<RecentItem>> {
    let st = state.inner().clone();
    // The browser's list is capped, but a hand-edited one needn't be.
    let items = items.into_iter().take(recent::MAX_LIVE).collect();
    blocking(move || Ok(st.db.write(move |t| recent::import(t, items, now_ms()))?)).await
}
