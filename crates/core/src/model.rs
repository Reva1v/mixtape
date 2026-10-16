//! Entities from data-model.md as they travel through the core and to the UI.

use crate::ids::{AlbumId, ArtistId, PlaylistId, TrackId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a track's audio lives. See the state diagram in data-model.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TrackState {
    /// Known from the catalog only.
    Remote,
    /// Audio sits in the streaming cache and can be evicted.
    Cached,
    /// Audio is in the library for good.
    Downloaded,
    /// Was downloaded, but the file is missing or unreadable.
    Broken,
}

impl TrackState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Cached => "cached",
            Self::Downloaded => "downloaded",
            Self::Broken => "broken",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "remote" => Self::Remote,
            "cached" => Self::Cached,
            "downloaded" => Self::Downloaded,
            "broken" => Self::Broken,
            _ => return None,
        })
    }

    /// Library screens show only tracks the user owns.
    pub fn in_library(self) -> bool {
        matches!(self, Self::Downloaded | Self::Broken)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArtistRef {
    #[ts(optional)]
    pub id: Option<ArtistId>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlbumRef {
    pub id: AlbumId,
    pub title: String,
}

/// A track row as lists and the player show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Track {
    pub id: TrackId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    #[ts(optional)]
    pub album: Option<AlbumRef>,
    #[ts(optional)]
    pub track_no: Option<u32>,
    #[ts(type = "number")]
    pub duration_ms: u64,
    #[ts(optional)]
    pub cover_key: Option<String>,
    pub state: TrackState,
    pub liked: bool,
}

impl Track {
    pub fn artist_names(&self) -> String {
        self.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AlbumType {
    Album,
    Ep,
    Single,
}

impl AlbumType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Ep => "ep",
            Self::Single => "single",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "ep" => Self::Ep,
            "single" => Self::Single,
            _ => Self::Album,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Album {
    pub id: AlbumId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    #[ts(optional)]
    pub year: Option<u16>,
    pub album_type: AlbumType,
    #[ts(optional)]
    pub cover_key: Option<String>,
    /// 4–5 hex colours extracted from the cover.
    pub palette: Vec<String>,
    #[ts(optional)]
    pub track_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Artist {
    pub id: ArtistId,
    pub name: String,
    #[ts(optional)]
    pub image_key: Option<String>,
    pub in_library: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PlaylistOrigin {
    Ytm,
    Local,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Playlist {
    pub id: PlaylistId,
    pub title: String,
    pub origin: PlaylistOrigin,
    pub track_count: u32,
    #[ts(optional)]
    pub cover_key: Option<String>,
}
