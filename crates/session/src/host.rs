//! What differs between the apps that run a session: the Mixtape app (its own window,
//! tray, thumbnail buttons) and the island's music module (no window of its own until
//! asked). Each implements [`Host`]; the session asks it instead of reaching for windows.

use crate::handoff::Peer;
use mixtape_core::Track;
use mixtape_player::PlaybackState;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// The Mixtape app.
    Standalone,
    /// The island's music module.
    Island,
}

/// The hand-off channel, as the host hears about it.
#[derive(Debug, Clone, PartialEq)]
pub enum HandoffEvent {
    /// A link to the other app is up.
    Connected(Peer),
    /// The playback went to the other app (it answered `ack{ok}`).
    Gave,
    /// The playback came from the other app; `playing` when it plays on here.
    Took { playing: bool },
    /// The link is gone: `bye` when the other app said goodbye, otherwise it vanished.
    PeerLeft { bye: bool },
}

pub trait Host: Send + Sync + 'static {
    fn kind(&self) -> HostKind;

    /// Label of the window with Mixtape's interface, if there is one; the session's events
    /// go there (the Mixtape app sends them to every window, as before).
    fn ui_window(&self) -> Option<String>;

    /// Opens or raises the full interface, on `screen` when given (`"login"`, …).
    fn show_ui(&self, app: &AppHandle, screen: Option<&str>);

    /// The window SMTC (media overlay and keys) is attached to.
    fn smtc_hwnd(&self, app: &AppHandle) -> Option<isize>;

    /// Taskbar thumbnail buttons on [`Host::smtc_hwnd`]'s window.
    fn taskbar_buttons(&self) -> bool;

    /// A sidecar program (`yt-dlp`, `deno`): where its executable is.
    fn sidecar(&self, name: &str) -> PathBuf;

    /// Commands of the app itself (not of the plugin) the interface may call: the window,
    /// mini player, tray, updates. The interface asks with `plugin:mixtape|host_caps`.
    fn caps(&self) -> Vec<String> {
        Vec::new()
    }

    fn on_handoff(&self, _app: &AppHandle, _ev: HandoffEvent) {}

    /// Every player state (the tray icon follows it).
    fn on_state(&self, _app: &AppHandle, _s: &PlaybackState) {}

    /// A new current track.
    fn on_track(&self, _app: &AppHandle, _t: &Track) {}
}

/// The host, managed by the plugin before the session exists.
#[derive(Clone)]
pub struct HostRef(pub Arc<dyn Host>);

pub fn host(app: &AppHandle) -> Option<Arc<dyn Host>> {
    app.try_state::<HostRef>().map(|h| h.0.clone())
}

/// Sends a session event to the interface: to every window in the Mixtape app (as it always
/// did), only to [`Host::ui_window`] in the island, nowhere when that has none.
pub fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    match host(app) {
        Some(h) if h.kind() == HostKind::Island => {
            if let Some(label) = h.ui_window() {
                let _ = app.emit_to(label.as_str(), event, payload);
            }
        }
        _ => {
            let _ = app.emit(event, payload);
        }
    }
}

/// The interface window is minimized or hidden (or there is none to look at).
pub fn ui_unseen(app: &AppHandle) -> bool {
    let Some(label) = host(app).and_then(|h| h.ui_window()) else {
        return false;
    };
    app.get_webview_window(&label)
        .map(|w| w.is_minimized().unwrap_or(false) || !w.is_visible().unwrap_or(true))
        .unwrap_or(false)
}
