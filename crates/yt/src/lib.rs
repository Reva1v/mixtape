//! Everything that talks to YouTube Music, behind one boundary (constitution IV):
//! cookies and request signing, the InnerTube client and its parsers, stream URLs via
//! the yt-dlp sidecar, cover URLs and request pacing.

pub mod api;
pub mod auth;
pub mod direct;
pub mod home;
pub mod innertube;
pub mod limits;
pub mod loudness;
pub mod meta;
pub mod parse;
pub mod radio;
pub mod resolver;
pub mod search;
pub mod thumbs;

use mixtape_core::{AppError, ErrorKind, tr};

#[derive(Debug, thiserror::Error)]
pub enum YtError {
    #[error("{}: {}", tr("No connection to YouTube", "Немає з'єднання з YouTube", "нет соединения с YouTube"), .0)]
    Network(String),
    /// Cookies are missing, expired or rejected — the UI asks to reconnect.
    #[error("{}", tr(
        "YouTube Music sign-in expired, connect the account again",
        "Вхід у YouTube Music закінчився, підключіть акаунт знову",
        "вход в YouTube Music истёк, подключите аккаунт заново",
    ))]
    Auth,
    /// YouTube throttles us (HTTP 429 or "confirm you're not a bot").
    #[error("{}", tr(
        "YouTube is limiting requests for now, we'll try again later",
        "YouTube тимчасово обмежив запити, спробуємо пізніше",
        "YouTube временно ограничил запросы, попробуем позже",
    ))]
    RateLimited,
    /// This item cannot be played or downloaded; the text says why, in the app language.
    #[error("{0}")]
    Unavailable(String),
    #[error("{}: {}", tr("Couldn't read the YouTube response", "Не вдалося розібрати відповідь YouTube", "не удалось разобрать ответ YouTube"), .0)]
    Parse(String),
    #[error("yt-dlp: {0}")]
    Tool(String),
}

pub type Result<T> = std::result::Result<T, YtError>;

impl From<reqwest::Error> for YtError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<YtError> for AppError {
    fn from(e: YtError) -> Self {
        let kind = match e {
            YtError::Network(_) => ErrorKind::Network,
            YtError::Auth => ErrorKind::Auth,
            YtError::RateLimited | YtError::Unavailable(_) | YtError::Parse(_) | YtError::Tool(_) => {
                ErrorKind::YouTube
            }
        };
        AppError::new(kind, e.to_string())
    }
}
