//! System Media Transport Controls: the Windows media overlay, lock-screen card and
//! hardware media keys, bound to the app window via `ISystemMediaTransportControlsInterop`.
//!
//! Without a `MediaPlayer` the app drives everything itself: enables the buttons,
//! handles `ButtonPressed`, publishes seek bounds so `PlaybackPositionChangeRequested`
//! fires, and calls `DisplayUpdater().Update()` after every metadata change.

use crate::{Error, MediaCommand};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
    SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs,
    SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::StorageFile;
use windows::Storage::Streams::RandomAccessStreamReference;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
use windows::core::{HSTRING, Ref, factory};

type Callback = Arc<dyn Fn(MediaCommand) + Send + Sync>;

/// SMTC session of one window.
///
/// `Send + Sync`: the WinRT objects are agile, so every method may be called from any
/// thread. `on_command` runs on a system thread-pool thread.
pub struct Smtc {
    controls: SystemMediaTransportControls,
    tokens: [i64; 2],
    /// Bumped on every metadata change and held while the display is updated, so a
    /// cover that finishes loading late never lands on the next track.
    generation: Arc<Mutex<u64>>,
}

impl Smtc {
    pub fn new(
        hwnd: isize,
        on_command: impl Fn(MediaCommand) + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let interop =
            factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        let controls: SystemMediaTransportControls =
            unsafe { interop.GetForWindow(HWND(hwnd as *mut _)) }?;
        controls.SetIsEnabled(true)?;
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        controls.SetIsNextEnabled(true)?;
        controls.SetIsPreviousEnabled(true)?;
        controls.SetIsStopEnabled(true)?;
        // Not `Closed`: media keys are routed only to a live session.
        controls.SetPlaybackStatus(MediaPlaybackStatus::Stopped)?;
        let updater = controls.DisplayUpdater()?;
        updater.SetType(MediaPlaybackType::Music)?;
        updater.Update()?;

        let on_command: Callback = Arc::new(on_command);
        let cb = on_command.clone();
        let pressed = controls.ButtonPressed(&TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(
            move |_, args: Ref<SystemMediaTransportControlsButtonPressedEventArgs>| {
                if let Some(cmd) = button_command(args.ok()?.Button()?) {
                    cb(cmd);
                }
                Ok(())
            },
        ))?;
        let cb = on_command;
        let seek = controls.PlaybackPositionChangeRequested(&TypedEventHandler::<
            SystemMediaTransportControls,
            PlaybackPositionChangeRequestedEventArgs,
        >::new(
            move |_, args: Ref<PlaybackPositionChangeRequestedEventArgs>| {
                let position_ms = to_ms(args.ok()?.RequestedPlaybackPosition()?);
                cb(MediaCommand::Seek { position_ms });
                Ok(())
            },
        ));
        let seek = match seek {
            Ok(token) => token,
            Err(e) => {
                let _ = controls.RemoveButtonPressed(pressed);
                return Err(e.into());
            }
        };
        Ok(Self {
            controls,
            tokens: [pressed, seek],
            generation: Arc::new(Mutex::new(0)),
        })
    }

    /// Shows a track. The cover (a local image file) loads asynchronously and appears
    /// shortly after the text; the call itself never waits on file I/O.
    pub fn set_metadata(
        &self,
        title: &str,
        artist: &str,
        album: &str,
        cover: Option<&Path>,
    ) -> Result<(), Error> {
        let mut generation = lock(&self.generation);
        *generation += 1;
        let updater = self.controls.DisplayUpdater()?;
        updater.SetType(MediaPlaybackType::Music)?;
        let props = updater.MusicProperties()?;
        props.SetTitle(&HSTRING::from(title))?;
        props.SetArtist(&HSTRING::from(artist))?;
        props.SetAlbumTitle(&HSTRING::from(album))?;
        updater.SetThumbnail(None)?;
        updater.Update()?;

        let expected = *generation;
        // Released before starting the load: `when` may run the callback inline.
        drop(generation);
        let Some(cover) = cover else { return Ok(()) };
        // StorageFile wants an absolute path without the `\\?\` prefix `canonicalize` adds.
        let path = std::path::absolute(cover).unwrap_or_else(|_| cover.to_path_buf());
        let (controls, shared) = (self.controls.clone(), self.generation.clone());
        StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_path()))?.when(move |file| {
            let apply = || -> windows::core::Result<()> {
                let file = file?;
                let generation = lock(&shared);
                if *generation != expected {
                    return Ok(());
                }
                let updater = controls.DisplayUpdater()?;
                updater.SetThumbnail(&RandomAccessStreamReference::CreateFromFile(&file)?)?;
                updater.Update()
            };
            if let Err(e) = apply() {
                tracing::warn!(path = %path.display(), "smtc cover: {e}");
            }
        })?;
        Ok(())
    }

    pub fn set_playback(&self, playing: bool) -> Result<(), Error> {
        let status = if playing {
            MediaPlaybackStatus::Playing
        } else {
            MediaPlaybackStatus::Paused
        };
        Ok(self.controls.SetPlaybackStatus(status)?)
    }

    /// Position and length of the current track; also the seek range for the overlay.
    /// Windows doesn't extrapolate, so call it on seeks and periodically (~5 s).
    pub fn set_timeline(&self, position_ms: u64, duration_ms: u64) -> Result<(), Error> {
        let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
        let (start, end) = (to_timespan(0), to_timespan(duration_ms));
        timeline.SetStartTime(start)?;
        timeline.SetEndTime(end)?;
        timeline.SetMinSeekTime(start)?;
        timeline.SetMaxSeekTime(end)?;
        timeline.SetPosition(to_timespan(position_ms.min(duration_ms)))?;
        Ok(self.controls.UpdateTimelineProperties(&timeline)?)
    }

    /// Nothing is playing: drops the metadata but keeps the session, so the Play key
    /// still reaches the app.
    pub fn clear(&self) -> Result<(), Error> {
        let mut generation = lock(&self.generation);
        *generation += 1;
        let updater = self.controls.DisplayUpdater()?;
        updater.ClearAll()?;
        updater.SetType(MediaPlaybackType::Music)?;
        updater.Update()?;
        self.set_timeline(0, 0)?;
        Ok(self
            .controls
            .SetPlaybackStatus(MediaPlaybackStatus::Stopped)?)
    }
}

impl Drop for Smtc {
    fn drop(&mut self) {
        *lock(&self.generation) += 1;
        let c = &self.controls;
        let _ = c.RemoveButtonPressed(self.tokens[0]);
        let _ = c.RemovePlaybackPositionChangeRequested(self.tokens[1]);
        let _ = c.DisplayUpdater().and_then(|u| {
            u.ClearAll()?;
            u.Update()
        });
        let _ = c.SetPlaybackStatus(MediaPlaybackStatus::Closed);
        let _ = c.SetIsEnabled(false);
    }
}

fn lock(m: &Mutex<u64>) -> std::sync::MutexGuard<'_, u64> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn button_command(button: SystemMediaTransportControlsButton) -> Option<MediaCommand> {
    Some(match button {
        SystemMediaTransportControlsButton::Play => MediaCommand::Play,
        SystemMediaTransportControlsButton::Pause => MediaCommand::Pause,
        SystemMediaTransportControlsButton::Stop => MediaCommand::Stop,
        SystemMediaTransportControlsButton::Next => MediaCommand::Next,
        SystemMediaTransportControlsButton::Previous => MediaCommand::Previous,
        _ => return None,
    })
}

/// `TimeSpan` counts 100 ns ticks.
fn to_timespan(ms: u64) -> TimeSpan {
    TimeSpan {
        Duration: i64::try_from(ms.saturating_mul(10_000)).unwrap_or(i64::MAX),
    }
}

fn to_ms(span: TimeSpan) -> u64 {
    u64::try_from(span.Duration).unwrap_or(0) / 10_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timespan_round_trips_milliseconds() {
        assert_eq!(to_timespan(1_500).Duration, 15_000_000);
        assert_eq!(to_ms(to_timespan(123_456)), 123_456);
        assert_eq!(to_ms(TimeSpan { Duration: -5 }), 0);
        assert_eq!(to_timespan(u64::MAX).Duration, i64::MAX);
    }

    #[test]
    fn buttons_map_to_commands() {
        use SystemMediaTransportControlsButton as B;
        assert_eq!(button_command(B::Play), Some(MediaCommand::Play));
        assert_eq!(button_command(B::Pause), Some(MediaCommand::Pause));
        assert_eq!(button_command(B::Next), Some(MediaCommand::Next));
        assert_eq!(button_command(B::Previous), Some(MediaCommand::Previous));
        assert_eq!(button_command(B::Stop), Some(MediaCommand::Stop));
        assert_eq!(button_command(B::ChannelUp), None);
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn smtc_is_send_sync() {
        assert_send_sync::<Smtc>();
    }
}
