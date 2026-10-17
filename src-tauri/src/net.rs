//! Network status (T092): a light probe once a minute (every 10 s while offline). On a
//! change the UI gets `net:status`; coming back online flushes likes, tops up a running
//! mix and wakes the downloader. Local playback never depends on it.

use crate::state::AppState;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::Emitter;

const PROBE_URL: &str = "https://music.youtube.com/generate_204";

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

pub fn spawn(st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let online = probe(st.yt.http()).await;
            if st.online.swap(online, Ordering::SeqCst) != online {
                tracing::info!(online, "network status changed");
                let _ = st.app.emit("net:status", NetStatus { online });
                if online {
                    crate::commands::mix::flush_likes(&st);
                    crate::commands::mix::top_up(&st);
                    st.downloads.wake();
                }
            }
            tokio::time::sleep(Duration::from_secs(if online { 60 } else { 10 })).await;
        }
    });
}

#[tauri::command]
pub fn net_status(state: tauri::State<'_, Arc<AppState>>) -> bool {
    state.online.load(Ordering::SeqCst)
}
