//! User preferences (Settings screen).

use super::{CmdResult, blocking};
use crate::state::AppState;
use mixtape_library::settings;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use ts_rs::TS;

const KEY: &str = "prefs";

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Prefs {
    /// Artwork glow and full-screen backdrop blend into the next track's colours.
    pub smooth_background: bool,
    /// Tracks blend into each other (crossfade).
    pub crossfade: bool,
    /// Crossfade length for automatic transitions, seconds.
    pub crossfade_sec: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { smooth_background: true, crossfade: false, crossfade_sec: 6 }
    }
}

impl Prefs {
    pub fn load(st: &AppState) -> Self {
        settings::get(&st.db, KEY).ok().flatten().unwrap_or_default()
    }

    /// Crossfade in ms as the player wants it (0 = off, gapless).
    pub fn crossfade_ms(&self) -> u32 {
        if self.crossfade { self.crossfade_sec.clamp(1, 12) * 1000 } else { 0 }
    }
}

#[tauri::command]
pub async fn prefs_get(state: State<'_, Arc<AppState>>) -> CmdResult<Prefs> {
    let st = state.inner().clone();
    blocking(move || Ok(Prefs::load(&st))).await
}

#[tauri::command]
pub async fn prefs_set(state: State<'_, Arc<AppState>>, prefs: Prefs) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        settings::set(&st.db, KEY, &prefs)?;
        st.player.set_crossfade(prefs.crossfade_ms());
        Ok(())
    })
    .await
}
