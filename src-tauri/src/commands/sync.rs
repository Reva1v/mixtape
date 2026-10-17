//! Account, sync and downloads commands (contracts/ipc.md, «Синхронизация и загрузки»).

use super::{CmdResult, blocking};
use crate::account::{self, AccountStatus};
use crate::commands::library::emit_changed;
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind, Playlist, PlaylistId, PlaylistOrigin};
use mixtape_library::jobs::{self, FailedJob};
use mixtape_library::views::ViewKind;
use mixtape_sync::downloader::{DownloadsProgress, SyncStatus};
use mixtape_sync::sources::{self, ImportResult, SyncPlan, SyncStage};
use mixtape_sync::url_import;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tauri::ipc::Channel;
use tauri::{Emitter, State};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncPreview {
    pub liked: u32,
    pub playlists: Vec<Playlist>,
    pub albums: u32,
    pub artists: u32,
}

/// `download_retry` takes a job id or `"all-failed"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RetryTarget {
    Job(i64),
    /// Always `"all-failed"`.
    All(#[allow(dead_code)] String),
}

fn need_account(st: &AppState) -> CmdResult<()> {
    if st.yt.is_authenticated() {
        Ok(())
    } else {
        Err(AppError::new(ErrorKind::Auth, "Сначала подключите аккаунт YouTube Music"))
    }
}

/// Maps an auth failure to the `account:expired` event as well as the error.
fn yt_error(st: &AppState, e: mixtape_yt::YtError) -> AppError {
    if matches!(e, mixtape_yt::YtError::Auth) {
        let _ = st.app.emit("account:expired", ());
    }
    e.into()
}

#[tauri::command]
pub async fn account_status(state: State<'_, Arc<AppState>>) -> CmdResult<AccountStatus> {
    let st = state.inner().clone();
    blocking(move || Ok(account::status(&st))).await
}

#[tauri::command]
pub async fn account_connect_cookies(state: State<'_, Arc<AppState>>, cookies_txt: String) -> CmdResult<AccountStatus> {
    account::connect(state.inner(), &cookies_txt).await
}

/// Opens Google sign-in in a Mixtape window and connects the account it signs into.
#[tauri::command]
pub async fn account_connect_webview(state: State<'_, Arc<AppState>>) -> CmdResult<AccountStatus> {
    crate::login_window::sign_in(state.inner()).await
}

#[tauri::command]
pub async fn account_disconnect(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || account::disconnect(&st)).await
}

#[tauri::command]
pub async fn sync_preview(state: State<'_, Arc<AppState>>) -> CmdResult<SyncPreview> {
    let st = state.inner().clone();
    need_account(&st)?;
    let p = sources::preview(&*st.yt).await.map_err(|e| yt_error(&st, e))?;
    // Playlist covers are small; fetch the missing ones so the picker is not blank.
    let mut playlists = Vec::with_capacity(p.playlists.len());
    for card in p.playlists {
        let key = card.id.to_string();
        if !st.covers.exists(&key)
            && let Some(url) = &card.thumbnail
            && let Ok(bytes) = mixtape_yt::thumbs::download(st.yt.http(), url).await
        {
            let _ = st.covers.process(&key, &bytes);
        }
        playlists.push(Playlist {
            cover_key: st.covers.exists(&key).then_some(key),
            id: card.id,
            title: card.title,
            origin: PlaylistOrigin::Ytm,
            track_count: card.track_count.unwrap_or(0),
        });
    }
    Ok(SyncPreview { liked: p.liked as u32, playlists, albums: p.albums as u32, artists: p.artists as u32 })
}

fn sync_status(stage: &SyncStage) -> SyncStatus {
    let (label, done, total) = match stage {
        SyncStage::Liked => ("Читаю лайки", 0, 0),
        SyncStage::Playlist(_) => ("Читаю плейлисты", 0, 0),
        SyncStage::Albums { done, total } => ("Читаю альбомы", *done, *total),
        SyncStage::Artists { done, total } => ("Читаю исполнителей", *done, *total),
        SyncStage::Covers { done, total } => ("Скачиваю обложки", *done, *total),
        SyncStage::Saving => ("Сохраняю в библиотеку", 0, 0),
    };
    SyncStatus { label: label.into(), done: done as u32, total: total as u32 }
}

const PLAN_KEY: &str = "sync_plan";
/// The scheduled sync runs when the last one is older than this.
const AUTO_SYNC_AFTER_MS: i64 = 12 * 60 * 60 * 1000;

/// Runs the sync in the background; progress shows up through the downloads channel.
#[tauri::command]
pub async fn sync_start(
    state: State<'_, Arc<AppState>>,
    liked: bool,
    playlist_ids: Vec<PlaylistId>,
    albums: bool,
    artists: bool,
) -> CmdResult<()> {
    let st = state.inner().clone();
    need_account(&st)?;
    let plan = SyncPlan { liked, playlist_ids, albums, artists };
    mixtape_library::settings::set(&st.db, PLAN_KEY, &plan)?;
    run_sync(st, plan, false)
}

/// One sync at a time. A scheduled (`quiet`) run only logs its failure.
fn run_sync(st: Arc<AppState>, plan: SyncPlan, quiet: bool) -> CmdResult<()> {
    if st.syncing.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(ErrorKind::Invalid, "Синхронизация уже идёт"));
    }
    tauri::async_runtime::spawn(async move {
        let _ = st.db.write(|t| jobs::clear_done(t));
        st.downloads.set_sync(Some(sync_status(&SyncStage::Liked)));
        let result = sources::run(&*st.yt, st.yt.http(), &st.db, &st.covers, &plan, |stage| {
            st.downloads.set_sync(Some(sync_status(&stage)));
        })
        .await;
        st.syncing.store(false, Ordering::SeqCst);
        st.downloads.set_sync(None);
        match result {
            Ok(report) => {
                tracing::info!(tracks = report.tracks, queued = report.queued, errors = report.errors.len(), quiet, "sync done");
                for (what, err) in &report.errors {
                    tracing::warn!(what, err, "sync source failed");
                }
                emit_changed(&st.app, &[ViewKind::Tracks, ViewKind::Albums, ViewKind::Artists, ViewKind::Playlists]);
                st.downloads.wake();
                let n = mixtape_sync::artists::fetch_missing(
                    st.yt.clone(),
                    st.yt.http().clone(),
                    st.db.clone(),
                    st.covers.clone(),
                )
                .await;
                if n > 0 {
                    emit_changed(&st.app, &[ViewKind::Artists]);
                }
            }
            Err(e) => {
                let e = yt_error(&st, e);
                if quiet {
                    tracing::warn!(error = %e.message, "scheduled sync failed");
                } else {
                    crate::events::toast_error(&st.app, format!("Синхронизация не удалась: {}", e.message));
                }
            }
        }
    });
    Ok(())
}

/// Re-runs the last chosen sync when it is older than 12 hours (checked every 30 minutes,
/// first a few minutes after start), so new likes and releases arrive without a click.
pub fn spawn_auto_sync(st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(180)).await;
        loop {
            let due = st.yt.is_authenticated()
                && st.online.load(Ordering::SeqCst)
                && st
                    .db
                    .read(mixtape_library::ytm::last_synced_at)
                    .ok()
                    .flatten()
                    .is_none_or(|t| mixtape_library::repo::now_ms() - t > AUTO_SYNC_AFTER_MS);
            if due
                && let Ok(Some(plan)) = mixtape_library::settings::get::<SyncPlan>(&st.db, PLAN_KEY)
            {
                tracing::info!("scheduled sync");
                let _ = run_sync(st.clone(), plan, true);
            }
            tokio::time::sleep(std::time::Duration::from_secs(1800)).await;
        }
    });
}

#[tauri::command]
pub async fn import_url(state: State<'_, Arc<AppState>>, url: String) -> CmdResult<ImportResult> {
    let st = state.inner().clone();
    need_account(&st)?;
    let link = url_import::parse_link(&url)
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "Это не ссылка на трек, альбом, плейлист или исполнителя YouTube Music"))?;
    let result = sources::import_link(&*st.yt, st.yt.http(), &st.db, &st.covers, link)
        .await
        .map_err(|e| yt_error(&st, e))?;
    emit_changed(&st.app, &[ViewKind::Playlists]);
    st.downloads.wake();
    Ok(result)
}

#[tauri::command]
pub fn downloads_pause(state: State<'_, Arc<AppState>>) {
    state.downloads.pause();
}

#[tauri::command]
pub fn downloads_resume(state: State<'_, Arc<AppState>>) {
    state.downloads.resume();
}

#[tauri::command]
pub fn download_retry(state: State<'_, Arc<AppState>>, job_id: RetryTarget) {
    state.downloads.retry(match job_id {
        RetryTarget::Job(id) => Some(id),
        RetryTarget::All(_) => None,
    });
}

#[tauri::command]
pub async fn downloads_failed(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<FailedJob>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.db.read(jobs::failed_list)?)).await
}

#[tauri::command]
pub fn downloads_subscribe(state: State<'_, Arc<AppState>>, channel: Channel<DownloadsProgress>) {
    let _ = channel.send(state.downloads.snapshot());
    *state.downloads_channel.lock().unwrap_or_else(|e| e.into_inner()) = Some(channel);
}
