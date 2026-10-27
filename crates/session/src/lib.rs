//! Mixtape's app layer, shared by the Mixtape app and the Droplet Island music module: the
//! session state, player events, streaming, downloads, SMTC, Last.fm, Discord, the phone
//! sync, the `cover:` scheme and every interface command, as the Tauri plugin `mixtape`
//! (`plugin:mixtape|<command>`). What differs between the two apps goes through [`Host`].

pub mod account;
pub mod background;
pub mod commands;
pub mod cover_protocol;
pub mod discord;
pub mod downloads;
pub mod events;
pub mod handoff;
pub mod host;
pub mod lastfm;
pub mod login_window;
pub mod media;
pub mod net;
pub mod state;
pub mod streaming;

pub use host::{HandoffEvent, Host, HostKind, HostRef};
pub use state::Session;

use std::sync::Arc;
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Wry};

/// The app commands the host offers (`Host::caps`), for `ipc.hostHas` in the interface.
#[tauri::command]
fn host_caps(host: tauri::State<'_, HostRef>) -> Vec<String> {
    host.0.caps()
}

/// The plugin: manages the [`Host`] and the [`Session`] (opened in the plugin's setup,
/// before the app's own), serves `cover:` and registers the commands.
pub fn plugin(host: Arc<dyn Host>) -> TauriPlugin<Wry> {
    Builder::new("mixtape")
        .register_asynchronous_uri_scheme_protocol("cover", cover_protocol::handle)
        .setup(move |app, _api| {
            app.manage(HostRef(host.clone()));
            let session = Session::init(app, host).map_err(|e| e.to_string())?;
            app.manage(Arc::new(session));
            tracing::debug!("setup: session managed");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            host_caps,
            commands::library::library_view,
            commands::library::library_rows,
            commands::library::library_search,
            commands::library::track_get,
            commands::library::album_get,
            commands::library::artist_get,
            commands::library::playlist_get,
            commands::library::playlist_edit,
            commands::catalog::catalog_search,
            commands::catalog::catalog_more,
            commands::catalog::catalog_suggest,
            commands::catalog::artist_catalog,
            commands::catalog::artist_songs,
            commands::catalog::catalog_download,
            commands::mix::track_set_liked,
            commands::mix::track_download,
            commands::mix::mix_start,
            commands::mix::mix_select_chip,
            commands::mix::mix_continue_local,
            commands::home::home_get,
            commands::home::home_shelves,
            commands::home::mix_start_playlist,
            commands::player::player_subscribe,
            commands::player::play,
            commands::player::player_toggle,
            commands::player::player_next,
            commands::player::player_prev,
            commands::player::player_seek,
            commands::player::player_set_volume,
            commands::player::player_set_repeat,
            commands::player::player_set_shuffle,
            commands::player::queue_get,
            commands::player::queue_add,
            commands::player::queue_remove,
            commands::player::queue_move,
            commands::player::queue_clear,
            commands::player::queue_jump,
            commands::lyrics::lyrics_get,
            commands::stats::stats_get,
            commands::peer::peer_status,
            commands::peer::peer_pair_start,
            commands::peer::peer_pair_cancel,
            commands::peer::peer_unpair,
            commands::peer::peer_firewall_add,
            // A development helper (imports any folder by path): never in release builds.
            #[cfg(debug_assertions)]
            commands::dev::dev_import_folder,
            commands::settings::prefs_get,
            commands::settings::prefs_set,
            commands::settings::storage_get,
            commands::settings::storage_set_cache_limit,
            commands::settings::storage_set_autosave,
            commands::settings::cache_clear,
            net::net_status,
            commands::settings::open_data_dir,
            lastfm::integrations_status,
            lastfm::lastfm_connect,
            lastfm::lastfm_confirm,
            lastfm::lastfm_disconnect,
            commands::sync::account_status,
            commands::sync::account_connect_cookies,
            commands::sync::account_connect_webview,
            commands::sync::account_disconnect,
            commands::sync::sync_preview,
            commands::sync::sync_start,
            commands::sync::import_url,
            commands::spotify::spotify_import,
            commands::spotify::spotify_cancel,
            commands::spotify::spotify_download,
            commands::sync::downloads_pause,
            commands::sync::downloads_resume,
            commands::sync::download_retry,
            commands::sync::downloads_failed,
            commands::sync::downloads_subscribe,
        ])
        .build()
}
