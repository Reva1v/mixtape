//! Where Mixtape keeps its data: `%LOCALAPPDATA%\Mixtape`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
    /// Downloaded tracks the user owns.
    pub library: PathBuf,
    /// Streamed and prefetched tracks, evicted by LRU.
    pub cache: PathBuf,
    /// Cover thumbnails and palettes.
    pub covers: PathBuf,
    pub db: PathBuf,
}

impl DataPaths {
    pub fn at(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref().to_owned();
        let paths = Self {
            library: root.join("library"),
            cache: root.join("cache"),
            covers: root.join("covers"),
            db: root.join("mixtape.db"),
            root,
        };
        for dir in [&paths.root, &paths.library, &paths.cache, &paths.covers] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(paths)
    }

    /// `%LOCALAPPDATA%\Mixtape`, or `MIXTAPE_DATA_DIR` when set (budget runs on a
    /// synthetic library, never touching the real one).
    pub fn default_location() -> std::io::Result<Self> {
        if let Some(dir) = std::env::var_os("MIXTAPE_DATA_DIR") {
            return Self::at(PathBuf::from(dir));
        }
        let base = dirs::data_local_dir().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is not set")
        })?;
        Self::at(base.join("Mixtape"))
    }
}
