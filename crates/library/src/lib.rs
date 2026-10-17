//! Local library: SQLite storage, search and settings.

pub mod conn;
pub mod db;
pub mod integrity;
pub mod jobs;
pub mod normalize;
pub mod paths;
pub mod playlists;
pub mod repo;
pub mod search;
pub mod settings;
pub mod views;
pub mod ytm;

pub use conn::Db;
pub use paths::DataPaths;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("database worker stopped")]
    WorkerGone,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for mixtape_core::AppError {
    fn from(e: Error) -> Self {
        mixtape_core::AppError::new(
            mixtape_core::ErrorKind::Storage,
            format!("Ошибка библиотеки: {e}"),
        )
    }
}
