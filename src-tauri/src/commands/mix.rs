//! «Включить микс» (US3): starting a mix, switching its chips, keeping it fed as the queue
//! runs low, the offline fallback, likes sent to YouTube Music and «Скачать».

use super::{CmdResult, blocking};
use crate::commands::library::emit_changed;
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind, TrackId, TrackState, tr};
use mixtape_library::views::ViewKind;
use mixtape_library::{cache_lru, jobs, likes, repo};
use mixtape_player::{MixChip, MixPage};
use mixtape_sync::sources;
use mixtape_yt::YtError;
use mixtape_yt::radio::{RadioChip, RadioPage};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, State};

type St<'a> = State<'a, Arc<AppState>>;

/// Tracks a mix may still pick from the library when it runs dry offline.
const OFFLINE_BATCH: usize = 30;

fn need_account(st: &AppState) -> CmdResult<()> {
    if st.yt.is_authenticated() {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::Auth,
            tr(
                "Mixes are made by YouTube Music, connect the account first",
                "Мікси збирає YouTube Music — спершу підключіть акаунт",
                "Миксы собирает YouTube Music — сначала подключите аккаунт",
            ),
        ))
    }
}

pub(super) fn yt_error(st: &AppState, e: YtError) -> AppError {
    if matches!(e, YtError::Auth) {
        let _ = st.app.emit("account:expired", ());
    }
    e.into()
}

/// Stores the page's tracks (they stay `remote`) and turns it into the player's shape.
pub(super) async fn page_of(st: &AppState, radio: RadioPage) -> CmdResult<MixPage> {
    let tracks = sources::store_tracks(st.yt.http(), &st.db, &st.covers, radio.tracks).await?;
    Ok(MixPage {
        tracks,
        chips: radio
            .chips
            .into_iter()
            .map(|c| MixChip { id: c.id, title: c.title, playlist_id: c.playlist_id, params: c.params, selected: c.selected })
            .collect(),
        continuation: radio.continuation,
        playlist_id: radio.playlist_id,
    })
}

#[tauri::command]
pub async fn mix_start(state: St<'_>, track_id: TrackId) -> CmdResult<()> {
    let st = state.inner().clone();
    need_account(&st)?;
    let mut radio = st.yt.radio(&track_id).await.map_err(|e| yt_error(&st, e))?;
    // A clip seed comes back as its audio version first; keep one of the two.
    let seed_title = st.db.read(|c| repo::track(c, &track_id)).ok().flatten().map(|t| t.title.to_lowercase());
    if let Some(first) = radio.tracks.first()
        && first.id != track_id
        && seed_title.as_deref() == Some(first.title.to_lowercase().as_str())
    {
        radio.tracks.remove(0);
    }
    let page = page_of(&st, radio).await?;
    if page.tracks.is_empty() {
        return Err(AppError::new(
            ErrorKind::NotFound,
            tr(
                "YouTube Music didn't make a mix for this track",
                "YouTube Music не зібрав мікс для цього треку",
                "YouTube Music не собрал микс для этого трека",
            ),
        ));
    }
    blocking(move || {
        st.player.start_mix(track_id, page);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn mix_select_chip(state: St<'_>, chip_id: String) -> CmdResult<()> {
    let st = state.inner().clone();
    need_account(&st)?;
    let (seed, chip, generation) = st
        .player
        .mix_select_chip(&chip_id)
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, tr("The mix is already over", "Мікс уже закінчився", "Микс уже закончился")))?;
    let chip = RadioChip {
        id: chip.id,
        title: chip.title,
        playlist_id: chip.playlist_id,
        params: chip.params,
        selected: true,
    };
    let radio = st.yt.radio_by_chip(&seed, &chip).await.map_err(|e| yt_error(&st, e))?;
    let page = page_of(&st, radio).await?;
    blocking(move || {
        st.player.mix_accept_chip(generation, page);
        Ok(())
    })
    .await
}

/// Appends library tracks when the mix cannot continue online.
#[tauri::command]
pub async fn mix_continue_local(state: St<'_>) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        let queued = st.player.queued_ids();
        let ids = st.db.read(|c| repo::offline_mix_ids(c, &queued, OFFLINE_BATCH))?;
        if ids.is_empty() {
            return Err(AppError::new(
                ErrorKind::NotFound,
                tr("No downloaded tracks in the library", "У бібліотеці немає завантажених треків", "В библиотеке нет скачанных треков"),
            ));
        }
        st.player.edit_queue(|q| q.append_mix(ids));
        Ok(())
    })
    .await
}

/// Called on every queue change: fetches the next page when the running mix is low.
pub fn top_up(st: &Arc<AppState>) {
    let Some(req) = st.player.mix_begin_more() else { return };
    let st = st.clone();
    tauri::async_runtime::spawn(async move {
        let page = match st.yt.radio_more(&req.playlist_id, &req.continuation).await {
            Ok(radio) => page_of(&st, radio).await,
            Err(e) => Err(yt_error(&st, e)),
        };
        match page {
            Ok(page) => st.player.mix_accept_more(req.generation, page),
            Err(e) => {
                tracing::warn!(error = %e.message, "mix continuation failed");
                st.player.mix_more_failed(req.generation);
                if e.kind == ErrorKind::Network && st.player.remaining() <= 1 {
                    let _ = st.app.emit("mix:offline", ());
                }
            }
        }
    });
}

#[tauri::command]
pub async fn track_set_liked(state: St<'_>, id: TrackId, liked: bool) -> CmdResult<()> {
    let st = state.inner().clone();
    let st2 = st.clone();
    let root = st.paths.root.clone();
    // A liked track that is only cached moves into the library (T076).
    blocking(move || {
        Ok(st2.db.write(move |t| {
            likes::queue_like(t, &id, liked)?;
            if liked {
                cache_lru::promote(t, &root, &id)?;
            }
            Ok(())
        })?)
    })
    .await?;
    emit_changed(&st.app, &[ViewKind::Liked, ViewKind::Tracks, ViewKind::Albums]);
    flush_likes(&st);
    Ok(())
}

static FLUSHING: AtomicBool = AtomicBool::new(false);
/// A retry timer is armed (at most one).
static RETRY_ARMED: AtomicBool = AtomicBool::new(false);

/// Sends pending likes to YouTube Music in the background; one flush at a time. Runs on
/// a like, at start and when the network returns; while rows are left after a failure, a
/// single retry timer backs off by their attempts (1 min doubling to 6 h). Nothing runs
/// while the outbox is empty.
pub fn flush_likes(st: &Arc<AppState>) {
    if !st.yt.is_authenticated() || FLUSHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let st = st.clone();
    tauri::async_runtime::spawn(async move {
        let mut report = mixtape_sync::likes::flush(&*st.yt, &st.db).await;
        // Likes queued while that flush ran have not been tried yet.
        for _ in 0..3 {
            if report.auth_expired || st.db.read(likes::oldest_attempts).ok().flatten() != Some(0) {
                break;
            }
            report = mixtape_sync::likes::flush(&*st.yt, &st.db).await;
        }
        FLUSHING.store(false, Ordering::SeqCst);
        if report.auth_expired {
            let _ = st.app.emit("account:expired", ());
        } else {
            arm_like_retry(&st);
        }
        if report.sent + report.failed + report.dropped > 0 {
            tracing::info!(?report, "likes flushed");
        }
    });
}

/// Schedules the next flush while the outbox still has rows.
fn arm_like_retry(st: &Arc<AppState>) {
    let Ok(Some(attempts)) = st.db.read(likes::oldest_attempts) else { return };
    if RETRY_ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    let delay = likes::retry_delay(attempts);
    let st = st.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        RETRY_ARMED.store(false, Ordering::SeqCst);
        // Offline: `net.rs` flushes when the network is back.
        if st.online.load(Ordering::SeqCst) {
            flush_likes(&st);
        }
    });
}

/// «Скачать»: a cached track moves into the library at once, anything else is queued.
#[tauri::command]
pub async fn track_download(state: St<'_>, id: TrackId) -> CmdResult<()> {
    let st = state.inner().clone();
    let st2 = st.clone();
    let promoted = blocking(move || {
        let root = st2.paths.root.clone();
        Ok(st2.db.write(move |t| {
            if cache_lru::promote(t, &root, &id)? {
                return Ok(true);
            }
            let state = repo::track(t, &id)?.map(|t| t.state);
            if !matches!(state, Some(TrackState::Downloaded)) {
                jobs::enqueue(t, &id, "manual")?;
            }
            Ok(false)
        })?)
    })
    .await?;
    if promoted {
        emit_changed(&st.app, &[ViewKind::Tracks, ViewKind::Albums, ViewKind::Artists]);
    } else {
        st.downloads.wake();
    }
    Ok(())
}

/// Autosave (T076): a cached track played in full often enough moves into the library.
pub fn autosave(st: &Arc<AppState>, id: &TrackId, full_plays: u32) {
    let threshold = cache_lru::autosave_threshold(&st.db).unwrap_or(Some(cache_lru::DEFAULT_AUTOSAVE_PLAYS));
    if !cache_lru::should_autosave(full_plays, threshold) {
        return;
    }
    let root = st.paths.root.clone();
    let id = id.clone();
    match st.db.write(move |t| cache_lru::promote(t, &root, &id)) {
        Ok(true) => emit_changed(&st.app, &[ViewKind::Tracks, ViewKind::Albums, ViewKind::Artists]),
        Ok(false) => {}
        Err(e) => tracing::warn!(error = %e, "autosave failed"),
    }
}
