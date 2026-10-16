//! Rare global events sent to the UI with `emit` (contracts/ipc.md, «События»).

use mixtape_core::Track;
use serde::Serialize;
use ts_rs::TS;

/// `track:changed` — a new track became current.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TrackChanged {
    pub track: Track,
    /// Cover colours, most prominent first; empty when unknown.
    pub palette: Vec<String>,
}
