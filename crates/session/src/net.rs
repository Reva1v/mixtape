//! Network status (T092). Real requests tell it most of the time: every InnerTube call
//! reports whether YouTube answered, a connect failure marks the app offline at once and a
//! timeout asks for a probe. While offline a light probe runs after 10 s, backing off to
//! every 5 minutes (at once that slow while the window is minimized or hidden to the tray
//! and nothing plays); while online it runs only every 15 minutes, and not at all while
//! the window is out of sight and nothing plays. On a change the UI gets `net:status`; coming back online flushes likes, tops up
//! a running mix, wakes the downloader and the scheduled sync. Local playback never
//! depends on it.

use crate::state::Session;
use mixtape_yt::innertube::Reach;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

const PROBE_URL: &str = "https://music.youtube.com/generate_204";
/// First probe interval while offline; doubles up to [`OFFLINE_PROBE_MAX`].
const OFFLINE_PROBE: Duration = Duration::from_secs(10);
const OFFLINE_PROBE_MAX: Duration = Duration::from_secs(5 * 60);
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
fn set_online(st: &Arc<Session>, online: bool) {
    if st.online.swap(online, Ordering::SeqCst) == online {
        return;
    }
    tracing::info!(online, "network status changed");
    crate::host::emit(&st.app, "net:status", NetStatus { online });
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
fn on_reach(st: &Arc<Session>, reach: Reach) {
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

/// Minimized or hidden to the tray with nothing playing: no one would see the status.
fn idle(st: &Session) -> bool {
    let unseen = crate::host::ui_unseen(&st.app);
    use mixtape_player::Status;
    unseen
        && !st
            .running_player()
            .is_some_and(|p| matches!(p.state().status, Status::Playing | Status::Loading | Status::Buffering))
}

/// How long to wait before the next probe after `failed` probes in a row found no network.
fn offline_delay(failed: u32, idle: bool) -> Duration {
    if idle {
        return OFFLINE_PROBE_MAX;
    }
    OFFLINE_PROBE.saturating_mul(1 << failed.min(16)).min(OFFLINE_PROBE_MAX)
}

pub fn spawn(st: Arc<Session>) {
    let weak: Weak<Session> = Arc::downgrade(&st);
    st.yt.set_reach_hook(move |reach| {
        if let Some(st) = weak.upgrade() {
            on_reach(&st, reach);
        }
    });
    tauri::async_runtime::spawn(async move {
        // One check at start, so the UI knows from the beginning.
        let online = probe(st.yt.http()).await;
        set_online(&st, online);
        let mut failed = 0u32;
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
                // A real request that gets through (`Reach::Answered`) marks the app online
                // without waiting for this.
                tokio::time::sleep(offline_delay(failed, idle(&st))).await;
            }
            let online = probe(st.yt.http()).await;
            failed = if online { 0 } else { failed.saturating_add(1) };
            set_online(&st, online);
        }
    });
}

#[tauri::command]
pub fn net_status(state: tauri::State<'_, Arc<Session>>) -> bool {
    state.online.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_probes_back_off() {
        assert_eq!(offline_delay(0, false), OFFLINE_PROBE);
        assert_eq!(offline_delay(1, false), Duration::from_secs(20));
        assert_eq!(offline_delay(10, false), OFFLINE_PROBE_MAX);
        assert_eq!(offline_delay(u32::MAX, false), OFFLINE_PROBE_MAX);
        // Out of sight, nothing playing: no fast polling at all.
        assert_eq!(offline_delay(0, true), OFFLINE_PROBE_MAX);
    }
}
