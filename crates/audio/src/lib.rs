//! Audio engine: decoding (symphonia + libopus), resampling, gapless or crossfaded playback
//! through cpal (WASAPI shared), EBU R128 loudness and normalisation gain.
//!
//! The engine thread owns every decoder and pushes interleaved stereo f32 at the device rate
//! into a lock-free ring; the device callback only pops from it. See [`Engine`].

mod crossfade;
mod decoder;
mod engine;
mod gain;
mod loudness;
mod output;
mod pipeline;
mod resample;
mod timeline;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

pub use decoder::{ProbeInfo, probe};
pub use engine::{Engine, EngineEvent};
pub use gain::gain_for;
pub use loudness::{Loudness, measure_loudness};
pub use symphonia::core::io::MediaSource;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("decode: {0}")]
    Decode(#[from] symphonia::core::errors::Error),
    #[error("no playable audio track")]
    NoTrack,
    #[error("resample: {0}")]
    Resample(String),
    #[error("output device: {0}")]
    Output(String),
    #[error("loudness: {0}")]
    Loudness(#[from] ebur128::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Something the engine can play.
pub struct Source {
    /// Opaque id echoed back in [`EngineEvent`]s.
    pub id: String,
    pub open: SourceOpener,
    /// Normalisation gain from [`gain_for`].
    pub gain_db: f32,
    /// File extension (`"webm"`, `"m4a"`) to speed up probing.
    pub hint_ext: Option<String>,
    /// Whether the transition into this source may crossfade (see
    /// [`Engine::set_crossfade`]); `false` keeps it gapless, e.g. within an album.
    pub crossfade: bool,
}

/// Opens the byte stream on the engine thread, so a slow source never blocks the caller. It may
/// be called again: symphonia readers cannot seek back once they hit the end, so a seek after
/// that reopens the source.
#[derive(Clone)]
pub enum SourceOpener {
    File(PathBuf),
    /// Any seekable stream, e.g. a partially downloaded cache file.
    Custom(Arc<dyn Fn() -> std::io::Result<Box<dyn MediaSource>> + Send + Sync>),
}

impl Source {
    pub fn file(id: impl Into<String>, path: impl Into<PathBuf>, gain_db: f32) -> Self {
        let path = path.into();
        let hint_ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        Self {
            id: id.into(),
            open: SourceOpener::File(path),
            gain_db,
            hint_ext,
            crossfade: true,
        }
    }
}

impl SourceOpener {
    fn open(self) -> std::io::Result<Box<dyn MediaSource>> {
        match self {
            Self::File(path) => Ok(Box::new(std::fs::File::open(path)?)),
            Self::Custom(open) => open(),
        }
    }
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Source")
            .field("id", &self.id)
            .field("open", &self.open)
            .field("gain_db", &self.gain_db)
            .field("crossfade", &self.crossfade)
            .finish()
    }
}

impl fmt::Debug for SourceOpener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(p) => f.debug_tuple("File").field(p).finish(),
            Self::Custom(_) => f.write_str("Custom"),
        }
    }
}
