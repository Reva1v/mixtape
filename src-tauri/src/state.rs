//! Process-wide state handed to commands through `tauri::State<Arc<AppState>>`.

use mixtape_artwork::CoverStore;
use mixtape_library::views::ViewCache;
use mixtape_library::{DataPaths, Db};
use std::time::Instant;

pub struct AppState {
    pub paths: DataPaths,
    pub db: Db,
    pub covers: CoverStore,
    pub views: ViewCache,
    pub started: Instant,
}

impl AppState {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let started = Instant::now();
        let paths = DataPaths::default_location()?;
        let db = Db::open(&paths.db)?;
        let covers = CoverStore::new(&paths.covers);
        let report = mixtape_library::integrity::check(&db, &paths.root)?;
        tracing::info!(root = %paths.root.display(), ?report, "data directory ready");
        Ok(Self { paths, db, covers, views: ViewCache::default(), started })
    }
}
