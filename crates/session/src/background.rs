//! Housekeeping after start: pending likes go out, missing pictures of library artists
//! are fetched, and covers nothing refers to any more are deleted. Each runs once, a little
//! after start, so the window comes up first; none of them repeats on a timer.

use crate::commands;
use crate::state::Session;
use mixtape_library::views::ViewKind;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Startup work waits this long so it does not compete with the first screen.
const START_DELAY: Duration = Duration::from_secs(60);
/// Covers written within this long are never deleted: a card on screen or a sync still
/// saving may not be in the database yet.
const COVER_MIN_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub fn spawn(st: Arc<Session>) {
    commands::mix::flush_likes(&st);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(START_DELAY).await;
        if st.yt.is_authenticated() && st.online.load(Ordering::SeqCst) {
            fetch_artist_pictures(&st).await;
        }
        clean_covers_in_background(st);
    });
}

/// Pictures of library artists that have none and were not looked for within a week.
pub async fn fetch_artist_pictures(st: &Arc<Session>) {
    let n = mixtape_sync::artists::fetch_missing(st.yt.clone(), st.yt.http().clone(), st.db.clone(), st.covers.clone())
        .await;
    if n > 0 {
        commands::library::emit_changed(&st.app, &[ViewKind::Artists]);
    }
}

/// Runs [`clean_covers`] on a thread of its own in background mode.
pub fn clean_covers_in_background(st: Arc<Session>) {
    let spawned = std::thread::Builder::new().name("cover-cleanup".into()).spawn(move || {
        mixtape_win::enter_background_mode();
        clean_covers(&st);
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "cover cleanup not started");
    }
}

/// Deletes covers no row refers to, and the 1024 px size of covers outside the library.
pub fn clean_covers(st: &Session) {
    let keys = match st.db.read(mixtape_library::covers::keys_in_use) {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!(error = %e, "cover cleanup skipped");
            return;
        }
    };
    let mut keep = keys.any;
    keep.insert(crate::account::AVATAR_KEY.to_owned());
    match st.covers.cleanup(&keep, &keys.full, COVER_MIN_AGE) {
        Ok(r) if r.covers + r.full_files > 0 => {
            tracing::info!(covers = r.covers, full_files = r.full_files, mb = r.bytes / 1_048_576, "covers cleaned up");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "cover cleanup failed"),
    }
}
