mod account;
mod commands;
mod cover_protocol;
mod downloads;
mod events;
mod login_window;
mod media;
mod net;
mod state;
mod streaming;
mod window;

use std::sync::Arc;
use tauri::Manager;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,mixtape=debug".into()),
        )
        .init();

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
        .register_asynchronous_uri_scheme_protocol("cover", cover_protocol::handle)
        .setup(|app| {
            let state = Arc::new(state::AppState::init(app.handle())?);
            app.manage(state.clone());
            tracing::debug!("setup: state managed");
            account::restore(&state);
            // Pictures for artists synced before they were fetched, or that failed last time.
            if state.yt.is_authenticated() {
                let st = state.clone();
                tauri::async_runtime::spawn(async move {
                    let n = mixtape_sync::artists::fetch_missing(st.yt.clone(), st.yt.http().clone(), st.db.clone(), st.covers.clone()).await;
                    if n > 0 {
                        commands::library::emit_changed(&st.app, &[mixtape_library::views::ViewKind::Artists]);
                    }
                });
            }
            net::spawn(state.clone());
            // Likes made offline or before a failure go out now and then every few minutes.
            {
                let st = state.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        commands::mix::flush_likes(&st);
                        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                    }
                });
            }
            state.player.set_crossfade(commands::settings::Prefs::load(&state).crossfade_ms());
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
            commands::library::album_get,
            commands::library::artist_get,
            commands::library::playlist_get,
            commands::library::playlist_edit,
            commands::mix::track_set_liked,
            commands::mix::track_download,
            commands::mix::mix_start,
            commands::mix::mix_select_chip,
            commands::mix::mix_continue_local,
            commands::library::home_get,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Mixtape");
}
