//! Glue between the download worker and the app: audio analysis with our decoder,
//! progress to the UI channel, and batched `library:changed` events as tracks land.

use crate::commands::library::emit_changed;
use crate::state::AppState;
use mixtape_core::TrackId;
use mixtape_library::views::ViewKind;
use mixtape_sync::downloader::{Analysis, DownloadsProgress, Hooks};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Finished downloads are announced at most this often, so a running sync does not
/// make every open list reload per track.
const LIBRARY_EVENT_EVERY: Duration = Duration::from_millis(1500);

pub struct AppHooks {
    app: AppHandle,
    dirty: Arc<AtomicBool>,
}

impl AppHooks {
    pub fn new(app: &AppHandle) -> Arc<Self> {
        let dirty = Arc::new(AtomicBool::new(false));
        let (flag, handle) = (dirty.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(LIBRARY_EVENT_EVERY).await;
                if flag.swap(false, Ordering::SeqCst) {
                    emit_changed(&handle, &[ViewKind::Tracks, ViewKind::Liked, ViewKind::Albums, ViewKind::Artists]);
                }
            }
        });
        Arc::new(Self { app: app.clone(), dirty })
    }
}

impl Hooks for AppHooks {
    fn analyze(&self, path: &Path) -> Analysis {
        let sample_rate = mixtape_audio::probe(path).ok().map(|p| p.sample_rate);
        let loudness = mixtape_audio::measure_loudness(path).ok();
        Analysis { sample_rate, lufs: loudness.as_ref().map(|l| l.lufs), true_peak: loudness.map(|l| l.true_peak) }
    }

    fn progress(&self, p: &DownloadsProgress) {
        if let Some(st) = self.app.try_state::<Arc<AppState>>()
            && let Some(ch) = st.downloads_channel.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        {
            let _ = ch.send(p.clone());
        }
    }

    fn finished(&self, _track: &TrackId) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    fn auth_expired(&self) {
        let _ = self.app.emit("account:expired", ());
    }

    fn low_space(&self) {
        crate::events::toast_error(&self.app, "На диске меньше 2 ГБ — загрузки на паузе");
    }
}
