mod account;
mod background;
mod commands;
mod cover_protocol;
mod downloads;
mod events;
mod login_window;
mod media;
mod net;
mod state;
mod streaming;
mod updates;
mod window;

use std::sync::Arc;
use tauri::Manager;

/// A small runtime: the app mostly waits on the network and the disk, so a worker per
/// logical core only adds idle threads.
fn install_runtime() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .max_blocking_threads(32)
        .thread_name("mixtape-rt")
        .enable_all()
        .build()
        .expect("tokio runtime");
    tauri::async_runtime::set(rt.handle().clone());
    // Lives as long as the process.
    std::mem::forget(rt);
}

pub fn run() {
    let default_filter = if cfg!(debug_assertions) { "info,mixtape=debug" } else { "warn,mixtape=info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| default_filter.into()),
        )
        .init();
    install_runtime();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(window::frame_plugin())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol("cover", cover_protocol::handle)
        .setup(|app| {
            let state = Arc::new(state::AppState::init(app.handle())?);
            app.manage(state.clone());
            tracing::debug!("setup: state managed");
            let prefs = commands::settings::Prefs::load(&state);
            prefs.apply_language(&state);
            account::restore(&state);
            net::spawn(state.clone());
            commands::sync::spawn_auto_sync(state.clone());
            // Pending likes, artist pictures, unused covers: once, after start.
            background::spawn(state.clone());
            updates::spawn(state.clone());
            state.player.set_crossfade(prefs.crossfade_ms());
            tracing::debug!("setup: crossfade applied");
            // `setup` runs on the main (UI) thread, which the thumbnail toolbar needs.
            state.media.install(app.handle());
            tracing::debug!("setup: media installed");
            #[cfg(debug_assertions)]
            if let Ok(dir) = std::env::var("MIXTAPE_DEV_IMPORT") {
                let st = state.clone();
                std::thread::spawn(move || match commands::dev::import_dir(&st, &dir) {
                    Ok(n) => {
                        tracing::info!(n, "dev import done");
                        commands::library::emit_changed(&st.app, &[mixtape_library::views::ViewKind::Tracks]);
                    }
                    Err(e) => tracing::warn!(error = %e, "dev import failed"),
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event
                && let Some(st) = window.app_handle().try_state::<Arc<state::AppState>>()
            {
                st.player.save_snapshot(true);
            }
        })
        .invoke_handler(tauri::generate_handler![
            window::app_ready,
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
            commands::dev::dev_import_folder,
            commands::settings::prefs_get,
            commands::settings::prefs_set,
            commands::settings::storage_get,
            commands::settings::storage_set_cache_limit,
            commands::settings::storage_set_autosave,
            commands::settings::cache_clear,
            net::net_status,
            commands::settings::open_data_dir,
            commands::sync::account_status,
            commands::sync::account_connect_cookies,
            commands::sync::account_connect_webview,
            commands::sync::account_disconnect,
            commands::sync::sync_preview,
            commands::sync::sync_start,
            commands::sync::import_url,
            commands::sync::downloads_pause,
            commands::sync::downloads_resume,
            commands::sync::download_retry,
            commands::sync::downloads_failed,
            commands::sync::downloads_subscribe,
            updates::app_version,
            updates::update_check,
            updates::update_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Mixtape");
}
