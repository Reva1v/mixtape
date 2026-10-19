//! Audio engine: decoding (symphonia + libopus), resampling, gapless or crossfaded playback
//! through cpal (WASAPI shared), EBU R128 loudness and normalisation gain.
//!
//! The engine thread owns every decoder and pushes interleaved stereo f32 at the device rate
//! into a lock-free ring; the device callback only pops from it. See [`Engine`].

pub mod analysis;
mod crossfade;
mod decoder;
mod engine;
mod fft;
mod gain;
mod loudness;
mod output;
mod pipeline;
mod resample;
mod smart;
mod sparse;
mod timeline;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

pub use analysis::{ANALYSIS_VERSION, Key, TrackAnalysis, analyze_file};
pub use decoder::{ProbeInfo, drop_segment_indexes, probe};
#[cfg(feature = "bench")]
#[doc(hidden)]
pub use engine::bench;
pub use engine::{Engine, EngineEvent};
pub use gain::gain_for;
pub use loudness::{Loudness, measure_loudness};
pub use sparse::{READ_TIMEOUT, SparseCacheFile, SparseReader};
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
    /// A streamed source's bytes are not downloaded yet; the engine retries later.
    #[error("waiting for the download")]
    Buffering,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Something the engine can play.
#[derive(Clone)]
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
    /// Tempo, key and structure for smart transitions, once the track was analysed.
    pub analysis: Option<Arc<TrackAnalysis>>,
}

/// Opens the byte stream on the engine thread, so a slow source never blocks the caller. It may
/// be called again: symphonia readers cannot seek back once they hit the end, so a seek after
/// that reopens the source.
#[derive(Clone)]
pub enum SourceOpener {
    File(PathBuf),
    /// Any seekable stream. Its reads run on the engine thread, so they should not block.
    Custom(Arc<dyn Fn() -> std::io::Result<Box<dyn MediaSource>> + Send + Sync>),
    /// A file still being downloaded. The engine never waits inside its reads: while the bytes
    /// it needs are missing it skips decoding and reports [`EngineEvent::Buffering`].
    Sparse(Arc<SparseCacheFile>),
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
            analysis: None,
        }
    }

    /// A track streamed into `file`; `ext` is its container (`"webm"`, `"m4a"`).
    pub fn stream(
        id: impl Into<String>,
        file: Arc<SparseCacheFile>,
        ext: &str,
        gain_db: f32,
    ) -> Self {
        Self {
            id: id.into(),
            open: SourceOpener::Sparse(file),
            gain_db,
            hint_ext: Some(ext.to_ascii_lowercase()),
            crossfade: true,
            analysis: None,
        }
    }
}

impl SourceOpener {
    fn open(self) -> std::io::Result<Box<dyn MediaSource>> {
        match self {
            Self::File(path) => Ok(Box::new(std::fs::File::open(path)?)),
            Self::Custom(open) => open(),
            Self::Sparse(file) => {
                let mut r = file.reader();
                r.set_nonblocking(true);
                Ok(Box::new(r))
            }
        }
    }

    /// Whether opening can start without waiting for a download.
    fn ready(&self) -> bool {
        match self {
            Self::Sparse(file) => decoder::can_open(file),
            _ => true,
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
            .field("analysis", &self.analysis.is_some())
            .finish()
    }
}

impl fmt::Debug for SourceOpener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(p) => f.debug_tuple("File").field(p).finish(),
            Self::Custom(_) => f.write_str("Custom"),
            Self::Sparse(file) => f.debug_tuple("Sparse").field(&file.path()).finish(),
        }
    }
}
