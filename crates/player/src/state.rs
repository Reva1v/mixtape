//! Types the player exposes to the UI.

use crate::queue::Repeat;
use mixtape_core::{AlbumId, ArtistId, PlaylistId, TrackId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Status {
    #[default]
    Idle,
    Loading,
    Buffering,
    Playing,
    Paused,
    Error,
}

/// Sent to the UI at ≤10 Hz while playing and on every change.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlaybackState {
    #[ts(optional)]
    pub track_id: Option<TrackId>,
    pub status: Status,
    #[ts(type = "number")]
    pub position_ms: u64,
    #[ts(type = "number")]
    pub duration_ms: u64,
    pub volume: f32,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Unix ms when `position_ms` was sampled.
    #[ts(type = "number")]
    pub timestamp_ms: u64,
}

/// What the user asked to play.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
#[ts(export)]
pub enum PlaySource {
    Album { id: AlbumId },
    Playlist { id: PlaylistId },
    Artist { id: ArtistId },
    /// A library list the UI is showing (tracks or liked), by its view id.
    View {
        #[ts(type = "number")]
        view_id: u64,
        liked: bool,
    },
    Tracks { track_ids: Vec<TrackId> },
}

/// A mix filter chip ("Знакомое", "Популярное", …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Chip {
    pub id: String,
    pub title: String,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueEntry {
    #[ts(type = "number")]
    pub uid: u64,
    pub origin: crate::queue::Origin,
    pub track: mixtape_core::Track,
}

/// What the queue panel shows.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueView {
    pub items: Vec<QueueEntry>,
    #[ts(optional)]
    pub current: Option<u32>,
    pub context: crate::queue::QueueContext,
    /// Present while a mix is running.
    #[ts(optional)]
    pub chips: Option<Vec<Chip>>,
}
