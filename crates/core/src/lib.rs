//! Domain types shared by every Mixtape crate and exported to the UI as TypeScript.

pub mod ids;
pub mod l10n;
pub mod model;

pub use ids::{AlbumId, ArtistId, PlaylistId, TrackId};
pub use l10n::{Lang, tr};
pub use model::*;

/// Error type that crosses the IPC boundary: a stable kind for code and a message for people, in the interface language.
#[derive(Debug, thiserror::Error, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[error("{message}")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ErrorKind {
    NotFound,
    Network,
    Auth,
    YouTube,
    Storage,
    Audio,
    Invalid,
    Internal,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

pub type AppResult<T> = Result<T, AppError>;
