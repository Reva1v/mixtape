//! Windows shell integration: media overlay / media keys (SMTC) and the taskbar
//! thumbnail toolbar, DPAPI for stored secrets and the start-with-Windows entry. On other
//! targets everything compiles to no-ops.

pub mod autostart;
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

/// The two-letter code of the Windows display language (`ru`, `uk`, `en`…); empty when
/// it is not one the app knows about.
pub fn ui_language() -> &'static str {
    #[cfg(windows)]
    {
        // SAFETY: a plain Win32 query with no arguments.
        let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        // The low 10 bits are the primary language (LANG_RUSSIAN, LANG_UKRAINIAN…).
        match id & 0x3ff {
            0x09 => "en",
            0x19 => "ru",
            0x22 => "uk",
            0x23 => "be",
            _ => "",
        }
    }
    #[cfg(not(windows))]
    ""
}

/// Puts the calling thread in background mode (low CPU, I/O and memory priority) for
/// housekeeping that must not compete with playback or the UI. Use on a thread of its own.
pub fn enter_background_mode() {
    #[cfg(windows)]
    // SAFETY: plain Win32 calls on the current thread's pseudo-handle.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// Shows or hides the child windows of `parent` (an HWND) with the window class `class`.
/// Used for native overlays that a compact window layout must not have on top of it.
pub fn show_child_windows(parent: isize, class: &str, show: bool) {
    #[cfg(windows)]
    // SAFETY: plain Win32 calls; a stale or foreign handle only makes them fail.
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, SW_HIDE, SW_SHOWNA, ShowWindow};
        use windows::core::{HSTRING, PCWSTR};
        let parent = HWND(parent as *mut _);
        let class = HSTRING::from(class);
        let mut after = None;
        while let Ok(child) = FindWindowExW(Some(parent), after, &class, PCWSTR::null()) {
            let _ = ShowWindow(child, if show { SW_SHOWNA } else { SW_HIDE });
            after = Some(child);
        }
    }
    #[cfg(not(windows))]
    let _ = (parent, class, show);
}

/// Opens an `https://` link in the default browser. Returns whether the shell took it.
pub fn open_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    #[cfg(windows)]
    // SAFETY: both strings outlive the call; no window handle is passed.
    unsafe {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{HSTRING, PCWSTR, w};
        let r = ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        // Values above 32 mean success.
        r.0 as usize > 32
    }
    #[cfg(not(windows))]
    false
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
