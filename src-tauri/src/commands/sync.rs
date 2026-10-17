//! Account, sync and downloads commands (contracts/ipc.md, «Синхронизация и загрузки»).

use super::{CmdResult, blocking};
use crate::account::{self, AccountStatus};
use crate::commands::library::emit_changed;
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind, Playlist, PlaylistId, PlaylistOrigin, tr, trf};
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
        Err(AppError::new(
            ErrorKind::Auth,
            tr("Connect a YouTube Music account first", "Спершу підключіть акаунт YouTube Music", "Сначала подключите аккаунт YouTube Music"),
        ))
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
            && let Ok(bytes) = mixtape_yt::thumbs::download_small(st.yt.http(), url).await
        {
            // The sync itself fetches the full cover of a playlist the user picks.
            let _ = st.covers.process(&key, &bytes, mixtape_artwork::Detail::Small);
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
        SyncStage::Liked => (tr("Reading likes", "Читаю лайки", "Читаю лайки"), 0, 0),
        SyncStage::Playlist(_) => (tr("Reading playlists", "Читаю плейлисти", "Читаю плейлисты"), 0, 0),
        SyncStage::Albums { done, total } => (tr("Reading albums", "Читаю альбоми", "Читаю альбомы"), *done, *total),
        SyncStage::Artists { done, total } => {
            (tr("Reading artists", "Читаю виконавців", "Читаю исполнителей"), *done, *total)
        }
        SyncStage::Covers { done, total } => {
            (tr("Downloading covers", "Завантажую обкладинки", "Скачиваю обложки"), *done, *total)
        }
        SyncStage::Saving => (tr("Saving to the library", "Зберігаю в бібліотеку", "Сохраняю в библиотеку"), 0, 0),
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
        return Err(AppError::new(ErrorKind::Invalid, tr("Sync is already running", "Синхронізація вже триває", "Синхронизация уже идёт")));
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
                crate::background::fetch_artist_pictures(&st).await;
            }
            Err(e) => {
                let e = yt_error(&st, e);
                if quiet {
                    tracing::warn!(error = %e.message, "scheduled sync failed");
                } else {
                    let m = e.message;
                    crate::events::toast_error(
                        &st.app,
                        trf!("Sync failed: {m}", "Синхронізація не вдалася: {m}", "Синхронизация не удалась: {m}"),
                    );
                }
            }
        }
        // The schedule re-arms from the new `last_synced_at`.
        AUTO_SYNC.notify_one();
    });
    Ok(())
}

/// Wakes the scheduled sync: a sync finished, or the network came back.
static AUTO_SYNC: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Re-check the schedule now (e.g. the network is back after the sync was due).
pub fn nudge_auto_sync() {
    AUTO_SYNC.notify_one();
}

/// A due sync that could not finish is tried again after this long.
const AUTO_SYNC_RETRY: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Time until the scheduled sync is due (zero = now), or `None` when it cannot run:
/// no account, no saved plan, or offline. Those wait for [`nudge_auto_sync`].
fn auto_sync_wait(st: &AppState) -> Option<std::time::Duration> {
    if !st.yt.is_authenticated() || !st.online.load(Ordering::SeqCst) {
        return None;
    }
    mixtape_library::settings::get::<SyncPlan>(&st.db, PLAN_KEY).ok().flatten()?;
    let last = st.db.read(mixtape_library::ytm::last_synced_at).ok().flatten();
    let left = last.map_or(0, |t| t + AUTO_SYNC_AFTER_MS - mixtape_library::repo::now_ms()).max(0);
    Some(std::time::Duration::from_millis(left as u64))
}

/// Re-runs the last chosen sync 12 hours after the previous one, so new likes and releases
/// arrive without a click. Sleeps until it is due (one timer, re-armed after every sync);
/// nothing wakes it in between except a finished sync or the network coming back.
pub fn spawn_auto_sync(st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(180)).await;
        loop {
            match auto_sync_wait(&st) {
                None => AUTO_SYNC.notified().await,
                Some(wait) if !wait.is_zero() => {
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = AUTO_SYNC.notified() => {}
                    }
                }
                Some(_) => {
                    let Ok(Some(plan)) = mixtape_library::settings::get::<SyncPlan>(&st.db, PLAN_KEY) else { continue };
                    tracing::info!("scheduled sync");
                    if run_sync(st.clone(), plan, true).is_ok() || st.syncing.load(Ordering::SeqCst) {
                        // Until this sync (or the one already running) finishes.
                        AUTO_SYNC.notified().await;
                    }
                    // Still due: it failed or was cut short. Try again later, not in a loop.
                    if auto_sync_wait(&st).is_some_and(|w| w.is_zero()) {
                        tokio::select! {
                            _ = tokio::time::sleep(AUTO_SYNC_RETRY) => {}
                            _ = AUTO_SYNC.notified() => {}
                        }
                    }
                }
            }
        }
    });
}

#[tauri::command]
pub async fn import_url(state: State<'_, Arc<AppState>>, url: String) -> CmdResult<ImportResult> {
    let st = state.inner().clone();
    need_account(&st)?;
    let link = url_import::parse_link(&url)
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Invalid,
                tr(
                    "This isn't a link to a YouTube Music track, album, playlist or artist",
                    "Це не посилання на трек, альбом, плейлист чи виконавця YouTube Music",
                    "Это не ссылка на трек, альбом, плейлист или исполнителя YouTube Music",
                ),
            )
        })?;
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
