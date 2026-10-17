//! Network status (T092). Real requests tell it most of the time: every InnerTube call
//! reports whether YouTube answered, a connect failure marks the app offline at once and a
//! timeout asks for a probe. While offline a light probe runs every 10 s; while online it
//! runs only every 15 minutes, and not at all while the window is minimized and nothing
//! plays. On a change the UI gets `net:status`; coming back online flushes likes, tops up
//! a running mix, wakes the downloader and the scheduled sync. Local playback never
//! depends on it.

use crate::state::AppState;
use mixtape_yt::innertube::Reach;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tauri::{Emitter, Manager};

const PROBE_URL: &str = "https://music.youtube.com/generate_204";
/// Probe interval while offline.
const OFFLINE_PROBE: Duration = Duration::from_secs(10);
/// Probe interval while online: real requests usually notice trouble first.
const ONLINE_PROBE: Duration = Duration::from_secs(15 * 60);

/// Wakes the probe loop early: a request hinted that the network is gone.
static NUDGE: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[derive(Clone, Serialize)]
struct NetStatus {
    online: bool,
}

async fn probe(http: &mixtape_sync::reqwest::Client) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(8), http.head(PROBE_URL).send()).await,
        Ok(Ok(r)) if r.status().as_u16() < 500
    )
}

/// Records the status; on a change tells the UI and, when back online, restarts what
/// waited for the network.
fn set_online(st: &Arc<AppState>, online: bool) {
    if st.online.swap(online, Ordering::SeqCst) == online {
        return;
    }
    tracing::info!(online, "network status changed");
    let _ = st.app.emit("net:status", NetStatus { online });
    if online {
        crate::commands::mix::flush_likes(st);
        crate::lastfm::flush(st);
        crate::commands::mix::top_up(st);
        st.downloads.wake();
        crate::commands::sync::nudge_auto_sync();
    } else {
        // Switch the loop to fast probing.
        NUDGE.notify_one();
    }
}

/// What an InnerTube request said about the network.
fn on_reach(st: &Arc<AppState>, reach: Reach) {
    match reach {
        Reach::Answered => {
            if !st.online.load(Ordering::SeqCst) {
                set_online(st, true);
            }
        }
        Reach::Unreachable => set_online(st, false),
        Reach::TimedOut => NUDGE.notify_one(),
    }
}

/// Minimized with nothing playing: no one would see the status, skip the check.
fn idle(st: &AppState) -> bool {
    let minimized = st.app.get_webview_window("main").and_then(|w| w.is_minimized().ok()).unwrap_or(false);
    use mixtape_player::Status;
    minimized && !matches!(st.player.state().status, Status::Playing | Status::Loading | Status::Buffering)
}

pub fn spawn(st: Arc<AppState>) {
    let weak: Weak<AppState> = Arc::downgrade(&st);
    st.yt.set_reach_hook(move |reach| {
        if let Some(st) = weak.upgrade() {
            on_reach(&st, reach);
        }
    });
    tauri::async_runtime::spawn(async move {
        // One check at start, so the UI knows from the beginning.
        let online = probe(st.yt.http()).await;
        set_online(&st, online);
        loop {
            if st.online.load(Ordering::SeqCst) {
                let nudged = tokio::select! {
                    _ = tokio::time::sleep(ONLINE_PROBE) => false,
                    _ = NUDGE.notified() => true,
                };
                if !nudged && idle(&st) {
                    continue;
                }
            } else {
                tokio::time::sleep(OFFLINE_PROBE).await;
            }
            let online = probe(st.yt.http()).await;
            set_online(&st, online);
        }
    });
}

#[tauri::command]
pub fn net_status(state: tauri::State<'_, Arc<AppState>>) -> bool {
    state.online.load(Ordering::SeqCst)
}
