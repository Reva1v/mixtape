pub mod library;
pub mod dev;
pub mod player;

use mixtape_core::{AppError, ErrorKind};

pub type CmdResult<T> = Result<T, AppError>;

/// Runs blocking library work off the async runtime.
pub async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::new(ErrorKind::Internal, format!("Внутренняя ошибка: {e}")))?
}
