//! Process-wide state handed to commands through `tauri::State`.

use mixtape_artwork::CoverStore;
use mixtape_library::{DataPaths, Db};
use std::time::Instant;

pub struct AppState {
    pub paths: DataPaths,
    pub db: Db,
    pub covers: CoverStore,
    pub started: Instant,
}

impl AppState {
    pub fn init() -> Result<Self, Box<dyn std::error::Error>> {
        let started = Instant::now();
        let paths = DataPaths::default_location()?;
        let db = Db::open(&paths.db)?;
        let covers = CoverStore::new(&paths.covers);
        tracing::info!(root = %paths.root.display(), "data directory ready");
        Ok(Self { paths, db, covers, started })
    }
}
