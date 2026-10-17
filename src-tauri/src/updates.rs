//! Updates from GitHub Releases (`latest.json` next to the signed installer). A check runs
//! a minute after start and then once a day while the setting is on; a found version is
//! announced to the UI, which offers to install it. Installing downloads the installer,
//! runs it in passive mode and closes the app.

use crate::commands::CmdResult;
use crate::commands::settings::Prefs;
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;
use ts_rs::TS;

const FIRST_CHECK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateInfo {
    pub version: String,
    /// Release notes, if the release has any.
    #[ts(optional)]
    pub notes: Option<String>,
}

fn failed(e: impl std::fmt::Display) -> AppError {
    AppError::new(
        ErrorKind::Network,
        mixtape_core::trf!("Couldn't check for updates: {e}", "Не вдалося перевірити оновлення: {e}", "Не удалось проверить обновления: {e}"),
    )
}

async fn find(app: &AppHandle) -> CmdResult<Option<tauri_plugin_updater::Update>> {
    app.updater().map_err(failed)?.check().await.map_err(failed)
}

pub fn spawn(st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if Prefs::load(&st).auto_update {
                match find(&st.app).await {
                    Ok(Some(u)) => {
                        let _ = st.app.emit("update:available", UpdateInfo { version: u.version, notes: u.body });
                    }
                    Ok(None) => {}
                    // Offline, or the release feed isn't reachable: try again tomorrow.
                    Err(e) => tracing::info!("update check: {}", e.message),
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> CmdResult<Option<UpdateInfo>> {
    Ok(find(&app).await?.map(|u| UpdateInfo { version: u.version, notes: u.body }))
}

/// Downloads and starts the installer; reports `update:progress` {done, total}. The
/// installer closes the app, so on success this does not return to the UI.
#[tauri::command]
pub async fn update_install(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    let update = find(&app).await?.ok_or_else(|| {
        AppError::new(ErrorKind::NotFound, mixtape_core::tr("No update available", "Оновлень немає", "Обновлений нет"))
    })?;
    // The playback position is saved with the queue, so the new version picks up there.
    state.player.pause();
    let handle = app.clone();
    let mut done = 0u64;
    update
        .download_and_install(
            move |chunk, total| {
                done += chunk as u64;
                let _ = handle.emit("update:progress", serde_json::json!({ "done": done, "total": total }));
            },
            || {},
        )
        .await
        .map_err(failed)?;
    app.restart();
}
