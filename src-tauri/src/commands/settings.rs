//! User preferences (Settings screen).

use super::{CmdResult, blocking};
use crate::state::AppState;
use mixtape_core::Lang;
use mixtape_library::{cache_lru, settings};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use ts_rs::TS;

const KEY: &str = "prefs";

/// Look of the sidebar icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum IconStyle {
    /// Thin outlines (Lucide).
    #[default]
    Outline,
    /// Filled Solar icons, the original look.
    Solid,
    /// Two-tone Solar icons.
    Duotone,
    /// Filled rounded Phosphor icons.
    Rounded,
}

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
    /// «Включить микс» also opens the full-screen player.
    pub mix_fullscreen: bool,
    /// Tracks play at a similar loudness (loud ones turned down, like YouTube Music).
    pub normalize_volume: bool,
    /// «Умные переходы (бета)»: beat-matched DJ-style transitions between analysed tracks
    /// (like Apple Music's AutoMix); tracks are analysed in the background while it is on.
    pub smart_transitions: bool,
    /// Interface language; absent until chosen, then the Windows display language decides.
    #[ts(optional)]
    pub language: Option<Lang>,
    /// Look for a new version on start and once a day.
    pub auto_update: bool,
    pub icon_style: IconStyle,
    /// The window's close button hides it to the tray; the music keeps playing.
    pub close_to_tray: bool,
    /// Discord shows what plays ("Listening to …"); only in builds with a Discord app id.
    pub discord_presence: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { smooth_background: true, crossfade: false, crossfade_sec: 6, mix_fullscreen: false, language: None, auto_update: true, icon_style: IconStyle::Outline, close_to_tray: true, discord_presence: true, normalize_volume: true, smart_transitions: false }
    }
}

impl Prefs {
    /// Stored preferences with the language always filled in.
    pub fn load(st: &AppState) -> Self {
        let mut p: Self = settings::get(&st.db, KEY).ok().flatten().unwrap_or_default();
        p.language = Some(p.language.unwrap_or_else(|| Lang::from_locale(mixtape_win::ui_language())));
        p
    }

    /// Makes the core's own messages and YouTube's answers follow the chosen language.
    pub fn apply_language(&self, st: &AppState) {
        let lang = self.language.unwrap_or_default();
        mixtape_core::l10n::set_lang(lang);
        st.yt.set_language(lang.code());
    }

    /// Hands the playback preferences to the player (and the analyser behind smart
    /// transitions).
    pub fn apply_playback(&self, st: &AppState) {
        st.player.set_crossfade(self.crossfade_ms());
        st.player.set_normalization(self.normalize_volume);
        st.player.set_smart_transitions(self.smart_transitions);
        st.analyzer.set_enabled(self.smart_transitions);
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
        prefs.apply_playback(&st);
        crate::discord::set_enabled(&st, prefs.discord_presence);
        Prefs::load(&st).apply_language(&st);
        Ok(())
    })
    .await
}

/// The «Хранилище» section of settings.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StorageInfo {
    #[ts(type = "number")]
    pub cache_bytes: u64,
    #[ts(type = "number")]
    pub cache_limit: u64,
    /// Full plays before a streamed track is saved; absent = autosave off.
    #[ts(optional)]
    pub autosave_plays: Option<u32>,
    pub data_dir: String,
}

fn storage_info(st: &AppState) -> CmdResult<StorageInfo> {
    Ok(StorageInfo {
        cache_bytes: st.db.read(cache_lru::cache_size)?,
        cache_limit: cache_lru::cache_limit(&st.db)?,
        autosave_plays: cache_lru::autosave_threshold(&st.db)?,
        data_dir: st.paths.root.display().to_string(),
    })
}

#[tauri::command]
pub async fn storage_get(state: State<'_, Arc<AppState>>) -> CmdResult<StorageInfo> {
    let st = state.inner().clone();
    blocking(move || storage_info(&st)).await
}

/// Sets the cache limit and trims the cache to it right away.
#[tauri::command]
pub async fn storage_set_cache_limit(state: State<'_, Arc<AppState>>, bytes: u64) -> CmdResult<StorageInfo> {
    let st = state.inner().clone();
    blocking(move || {
        let bytes = bytes.max(512 * 1024 * 1024);
        settings::set(&st.db, cache_lru::CACHE_LIMIT_KEY, &bytes)?;
        trim(&st, bytes)?;
        storage_info(&st)
    })
    .await
}

#[tauri::command]
pub async fn storage_set_autosave(state: State<'_, Arc<AppState>>, plays: Option<u32>) -> CmdResult<StorageInfo> {
    let st = state.inner().clone();
    blocking(move || {
        settings::set(&st.db, cache_lru::AUTOSAVE_KEY, &plays.filter(|&n| n > 0))?;
        storage_info(&st)
    })
    .await
}

/// Deletes every cached stream except the playing track's, then covers left without a row.
#[tauri::command]
pub async fn cache_clear(state: State<'_, Arc<AppState>>) -> CmdResult<StorageInfo> {
    let st = state.inner().clone();
    blocking(move || {
        trim(&st, 0)?;
        crate::background::clean_covers_in_background(st.clone());
        storage_info(&st)
    })
    .await
}

fn trim(st: &AppState, limit: u64) -> CmdResult<()> {
    // The playing track and the one lined up after it: evicting the successor's file would
    // make it fail just before it is due.
    let protect = st.player.upcoming();
    let root = st.paths.root.clone();
    st.db.write(move |t| cache_lru::evict(t, &root, limit, &protect))?;
    Ok(())
}

#[tauri::command]
pub fn open_data_dir(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    std::process::Command::new("explorer")
        .arg(&state.paths.root)
        .spawn()
        .map(drop)
        .map_err(|e| mixtape_core::AppError::new(mixtape_core::ErrorKind::Internal, e.to_string()))
}
