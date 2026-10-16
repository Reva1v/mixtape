mod commands;
mod cover_protocol;
mod events;
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
            let state = state::AppState::init()?;
            app.manage(Arc::new(state));
            Ok(())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Mixtape");
}
