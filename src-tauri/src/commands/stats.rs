//! Listening statistics («Статистика»): a recap of the play log for a period.

use super::{CmdResult, blocking};
use crate::state::AppState;
use mixtape_library::repo;
use mixtape_library::stats::{self, ListeningStats, StatsPeriod};
use std::sync::Arc;
use tauri::State;

/// The recap of `period`. `tz_offset_min` is local time minus UTC in minutes (the
/// negated `Date.getTimezoneOffset()`), so hours and days are the user's local ones.
/// `device` (a `DeviceTotal.device`) keeps only that device's plays; `byDevice` always
/// covers all of them.
#[tauri::command]
pub async fn stats_get(
    state: State<'_, Arc<AppState>>,
    period: StatsPeriod,
    tz_offset_min: i32,
    device: Option<String>,
) -> CmdResult<ListeningStats> {
    let st = state.inner().clone();
    // A sane offset only: real ones are within ±14 h.
    let offset_ms = i64::from(tz_offset_min.clamp(-16 * 60, 16 * 60)) * 60_000;
    blocking(move || {
        Ok(st.db.read(|c| {
            let since = stats::period_start(c, period, repo::now_ms(), offset_ms)?;
            stats::listening_for(c, since, offset_ms, device.as_deref())
        })?)
    })
    .await
}
