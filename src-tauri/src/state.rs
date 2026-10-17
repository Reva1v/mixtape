//! Process-wide state handed to commands through `tauri::State<Arc<AppState>>`.

use crate::media::Media;
use mixtape_artwork::CoverStore;
use mixtape_library::views::ViewCache;
use mixtape_library::{DataPaths, Db};
use mixtape_player::{PlaybackState, Player};
use mixtape_sync::downloader::{Downloader, DownloadsProgress};
use mixtape_sync::lyrics::Lrclib;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::resolver::StreamResolver;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::AppHandle;
use tauri::ipc::Channel;

pub struct AppState {
    pub app: AppHandle,
    pub paths: DataPaths,
    pub db: Db,
    pub covers: CoverStore,
    pub views: ViewCache,
    pub player: Arc<Player>,
    /// Where `PlaybackState` updates go; set by `player_subscribe`.
    pub player_channel: Mutex<Option<Channel<PlaybackState>>>,
    /// Bumped by `spectrum_subscribe`/`spectrum_unsubscribe`; a spectrum loop runs while it
    /// still holds the value it started with.
    pub spectrum_generation: AtomicU64,
    pub media: Media,
    pub started: Instant,
    pub yt: Arc<InnerTube>,
    pub resolver: Arc<StreamResolver>,
    pub downloads: Downloader,
    /// Where `DownloadsProgress` updates go; set by `downloads_subscribe`.
    pub downloads_channel: Mutex<Option<Channel<DownloadsProgress>>>,
    /// A source sync is running (only one at a time).
    pub syncing: AtomicBool,
    /// Last network probe succeeded (see `net.rs`).
    pub online: AtomicBool,
    pub lrclib: Lrclib,
}

/// yt-dlp and deno sit next to the executable (Tauri `externalBin`); in a debug build
/// they may still be only in `src-tauri/binaries`.
fn sidecar(name: &str) -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let beside = exe_dir.join(format!("{name}.exe"));
    if beside.exists() || !cfg!(debug_assertions) {
        return beside;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries").join(format!("{name}-x86_64-pc-windows-msvc.exe"))
}

impl AppState {
    pub fn init(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let started = Instant::now();
        let paths = DataPaths::default_location()?;
        let db = Db::open(&paths.db)?;
        tracing::debug!(ms = started.elapsed().as_millis() as u64, "db open");
        let covers = CoverStore::new(&paths.covers);
        let report = mixtape_library::integrity::check(&db, &paths.root)?;
        tracing::debug!(ms = started.elapsed().as_millis() as u64, "integrity checked");
        let merged = db.write(|t| {
            let merged = mixtape_library::ytm::merge_local_twins(t)?;
            mixtape_library::ytm::link_artists_by_name(t)?;
            Ok(merged)
        })?;
        if merged > 0 {
            tracing::info!(merged, "local imports folded into their YouTube Music twins");
        }
        tracing::info!(root = %paths.root.display(), ?report, "data directory ready");
        let yt = Arc::new(InnerTube::new("ru", "RU")?);
        let tmp = paths.root.join("tmp");
        std::fs::create_dir_all(&tmp)?;
        let resolver = Arc::new(StreamResolver::new(sidecar("yt-dlp"), sidecar("deno"), tmp));
        let streams = crate::streaming::AppStreams::new(
            app.clone(),
            tauri::async_runtime::handle().inner().clone(),
            db.clone(),
            paths.root.clone(),
            resolver.clone(),
            yt.http().clone(),
        );
        let handle = app.clone();
        tracing::debug!("starting player");
        let player = Player::start(
            db.clone(),
            paths.root.clone(),
            Arc::new(move |ev| crate::events::on_player_event(&handle, ev)),
            Some(streams),
        )?;
        tracing::debug!("player started");
        let downloads = Downloader::start(
            tauri::async_runtime::handle().inner(),
            db.clone(),
            resolver.clone(),
            yt.http().clone(),
            paths.root.clone(),
            crate::downloads::AppHooks::new(app),
        );
        Ok(Self {
            app: app.clone(),
            paths,
            db,
            covers,
            views: ViewCache::default(),
            player,
            player_channel: Mutex::new(None),
            spectrum_generation: AtomicU64::new(0),
            media: Media::default(),
            started,
            yt,
            resolver,
            downloads,
            downloads_channel: Mutex::new(None),
            syncing: AtomicBool::new(false),
            online: AtomicBool::new(true),
            lrclib: Lrclib::new()?,
        })
    }
}
