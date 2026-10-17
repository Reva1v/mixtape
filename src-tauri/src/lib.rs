mod commands;
mod cover_protocol;
mod events;
mod media;
mod state;
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
            // `setup` runs on the main (UI) thread, which the thumbnail toolbar needs.
            state.media.install(app.handle());
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
            commands::library::track_set_liked,
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
            commands::dev::dev_import_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Mixtape");
}
