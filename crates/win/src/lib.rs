//! Windows shell integration: media overlay / media keys (SMTC) and the taskbar
//! thumbnail toolbar, and DPAPI for stored secrets. On other targets everything compiles to no-ops.

#[cfg(windows)]
pub mod dpapi;
mod glyph;
#[cfg(windows)]
mod smtc;
#[cfg(windows)]
mod thumbbar;

#[cfg(windows)]
pub use smtc::Smtc;
#[cfg(windows)]
pub use thumbbar::ThumbBar;

#[cfg(not(windows))]
pub use stub::{Smtc, ThumbBar};

/// A transport command from media keys, the media overlay or the thumbnail toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    Seek { position_ms: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[cfg(windows)]
    #[error("windows: {0}")]
    Windows(#[from] windows::core::Error),
}

#[cfg(not(windows))]
mod stub {
    use super::{Error, MediaCommand};
    use std::path::Path;

    pub struct Smtc;

    impl Smtc {
        pub fn new(
            _hwnd: isize,
            _on_command: impl Fn(MediaCommand) + Send + Sync + 'static,
        ) -> Result<Self, Error> {
            Ok(Self)
        }
        pub fn set_metadata(
            &self,
            _title: &str,
            _artist: &str,
            _album: &str,
            _cover: Option<&Path>,
        ) -> Result<(), Error> {
            Ok(())
        }
        pub fn set_playback(&self, _playing: bool) -> Result<(), Error> {
            Ok(())
        }
        pub fn set_timeline(&self, _position_ms: u64, _duration_ms: u64) -> Result<(), Error> {
            Ok(())
        }
        pub fn clear(&self) -> Result<(), Error> {
            Ok(())
        }
    }

    pub struct ThumbBar;

    impl ThumbBar {
        pub fn install(
            _hwnd: isize,
            _on_command: impl Fn(MediaCommand) + Send + Sync + 'static,
        ) -> Result<Self, Error> {
            Ok(Self)
        }
        pub fn set_playing(&self, _playing: bool) -> Result<(), Error> {
            Ok(())
        }
    }
}
