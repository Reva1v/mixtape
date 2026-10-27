//! Process-wide state handed to commands through `tauri::State<Arc<Session>>`.

use crate::host::Host;
use crate::media::Media;
use mixtape_artwork::CoverStore;
use mixtape_library::views::ViewCache;
use mixtape_library::{DataPaths, Db};
use mixtape_player::analyzer::AnalyzerHooks;
use mixtape_player::{Analyzer, PlaybackState, Player};
use mixtape_sync::downloader::{Downloader, DownloadsProgress};
use mixtape_sync::lyrics::Lrclib;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::resolver::StreamResolver;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tauri::AppHandle;
use tauri::ipc::Channel;

pub struct Session {
    pub app: AppHandle,
    /// The app running this session (window, tray, sidecars).
    pub host: Arc<dyn Host>,
    pub paths: DataPaths,
    pub db: Db,
    pub covers: CoverStore,
    pub views: ViewCache,
    pub player: Arc<Player>,
    /// Background track analysis for smart transitions (on while they are).
    pub analyzer: Analyzer,
    /// Where `PlaybackState` updates go; set by `player_subscribe`.
    pub player_channel: Mutex<Option<Channel<PlaybackState>>>,
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
    /// Stats sync with the phone: the server and the pairing code on screen.
    pub peer: crate::commands::peer::PeerState,
    lrclib: OnceLock<Lrclib>,
}

/// Settings flag: the one-time fold of local imports into their YouTube twins is done.
const LOCAL_TWINS_KEY: &str = "local_twins_merged";

impl Session {
    pub fn init(app: &AppHandle, host: Arc<dyn Host>) -> Result<Self, Box<dyn std::error::Error>> {
        let started = Instant::now();
        let paths = DataPaths::default_location()?;
        let db = Db::open(&paths.db)?;
        tracing::debug!(ms = started.elapsed().as_millis() as u64, "db open");
        let covers = CoverStore::new(&paths.covers);
        let report = mixtape_library::integrity::check(&db, &paths.root)?;
        tracing::debug!(ms = started.elapsed().as_millis() as u64, "integrity checked");
        // Once per database: later imports and syncs link their own credits.
        if !mixtape_library::settings::get_or(&db, LOCAL_TWINS_KEY, false)? {
            let merged = db.write(|t| {
                let merged = mixtape_library::ytm::merge_local_twins(t)?;
                mixtape_library::ytm::link_artists_by_name(t)?;
                Ok(merged)
            })?;
            mixtape_library::settings::set(&db, LOCAL_TWINS_KEY, &true)?;
            if merged > 0 {
                tracing::info!(merged, "local imports folded into their YouTube Music twins");
            }
        }
        tracing::info!(root = %paths.root.display(), ?report, "data directory ready");
        let yt = Arc::new(InnerTube::new("ru", "RU")?);
        let tmp = paths.root.join("tmp");
        std::fs::create_dir_all(&tmp)?;
        let resolver = Arc::new(StreamResolver::new(host.sidecar("yt-dlp"), host.sidecar("deno"), tmp));
        let streams = crate::streaming::AppStreams::new(
            app.clone(),
            tauri::async_runtime::handle().inner().clone(),
            db.clone(),
            paths.root.clone(),
            resolver.clone(),
            yt.clone(),
        );
        let handle = app.clone();
        tracing::debug!("starting player");
        let player = Player::start(
            db.clone(),
            paths.root.clone(),
            Arc::new(move |ev| crate::events::on_player_event(&handle, ev)),
            Some(streams.clone()),
        )?;
        streams.set_player(&player);
        tracing::debug!("player started");
        let analyzer = Analyzer::start(
            db.clone(),
            paths.root.clone(),
            AnalyzerHooks::for_player(&player, mixtape_win::enter_background_mode),
        )?;
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
            host,
            paths,
            db,
            covers,
            views: ViewCache::default(),
            player,
            analyzer,
            player_channel: Mutex::new(None),
            media: Media::default(),
            started,
            yt,
            resolver,
            downloads,
            downloads_channel: Mutex::new(None),
            syncing: AtomicBool::new(false),
            online: AtomicBool::new(true),
            peer: Default::default(),
            lrclib: OnceLock::new(),
        })
    }

    /// The lrclib client, built on first use (most sessions never open lyrics).
    pub fn lrclib(&self) -> Result<&Lrclib, mixtape_core::AppError> {
        if let Some(l) = self.lrclib.get() {
            return Ok(l);
        }
        let l = Lrclib::new()
            .map_err(|e| mixtape_core::AppError::new(mixtape_core::ErrorKind::Internal, e.to_string()))?;
        Ok(self.lrclib.get_or_init(|| l))
    }
}
