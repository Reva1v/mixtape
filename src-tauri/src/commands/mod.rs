pub mod catalog;
pub mod library;
pub mod dev;
pub mod home;
pub mod mix;
pub mod peer;
pub mod lyrics;
pub mod player;
pub mod settings;
pub mod spotify;
pub mod stats;
pub mod sync;

use mixtape_core::{AppError, ErrorKind, trf};

pub type CmdResult<T> = Result<T, AppError>;

/// Runs blocking library work off the async runtime.
pub async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::new(ErrorKind::Internal, trf!("Internal error: {e}", "Внутрішня помилка: {e}", "Внутренняя ошибка: {e}")))?
}
