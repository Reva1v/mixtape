//! Local library: SQLite storage, search and settings.

pub mod analysis;
pub mod cache_lru;
pub mod conn;
pub mod covers;
pub mod db;
pub mod integrity;
pub mod jobs;
pub mod likes;
pub mod lyrics;
pub mod normalize;
pub mod paths;
pub mod peer_store;
pub mod picks;
pub mod playlists;
pub mod recent;
pub mod repo;
pub mod scrobbles;
pub mod search;
pub mod settings;
pub mod stats;
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
    /// The database was moved on by a newer Mixtape: this build must not touch it.
    #[error("the library was made by a newer Mixtape (schema {found}, this version knows {known})")]
    TooNew { found: u32, known: u32 },
    #[error("database worker stopped")]
    WorkerGone,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for mixtape_core::AppError {
    fn from(e: Error) -> Self {
        if let Error::TooNew { .. } = e {
            return mixtape_core::AppError::new(
                mixtape_core::ErrorKind::Storage,
                mixtape_core::tr(
                    "Your library was updated by a newer version of Mixtape. Install the latest version from github.com/Reva1v/mixtape/releases.",
                    "Вашу бібліотеку оновила новіша версія Mixtape. Встановіть останню версію з github.com/Reva1v/mixtape/releases.",
                    "Вашу библиотеку обновила более новая версия Mixtape. Установите последнюю версию с github.com/Reva1v/mixtape/releases."
                ),
            );
        }
        mixtape_core::AppError::new(
            mixtape_core::ErrorKind::Storage,
            mixtape_core::trf!("Library error: {e}", "Помилка бібліотеки: {e}", "Ошибка библиотеки: {e}"),
        )
    }
}
