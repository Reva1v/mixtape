//! Process-wide state handed to commands through `tauri::State<Arc<AppState>>`.

use crate::media::Media;
use mixtape_artwork::CoverStore;
use mixtape_library::views::ViewCache;
use mixtape_library::{DataPaths, Db};
use mixtape_player::{PlaybackState, Player};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::AppHandle;
use tauri::ipc::Channel;

pub struct AppState {
    pub app: AppHandle,
    #[allow(dead_code)] // used by downloads (US2)
    pub paths: DataPaths,
    pub db: Db,
    pub covers: CoverStore,
    pub views: ViewCache,
    pub player: Arc<Player>,
    /// Where `PlaybackState` updates go; set by `player_subscribe`.
    pub player_channel: Mutex<Option<Channel<PlaybackState>>>,
    pub media: Media,
    pub started: Instant,
}

impl AppState {
    pub fn init(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let started = Instant::now();
        let paths = DataPaths::default_location()?;
        let db = Db::open(&paths.db)?;
        let covers = CoverStore::new(&paths.covers);
        let report = mixtape_library::integrity::check(&db, &paths.root)?;
        tracing::info!(root = %paths.root.display(), ?report, "data directory ready");
        let handle = app.clone();
        tracing::debug!("starting player");
        let player = Player::start(
            db.clone(),
            paths.root.clone(),
            Arc::new(move |ev| crate::events::on_player_event(&handle, ev)),
        )?;
        tracing::debug!("player started");
        Ok(Self {
            app: app.clone(),
            paths,
            db,
            covers,
            views: ViewCache::default(),
            player,
            player_channel: Mutex::new(None),
            media: Media::default(),
            started,
        })
    }
}
