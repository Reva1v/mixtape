mod autostart;
mod host;
mod links;
mod mini;
mod tray;
mod tray_menu;
mod updates;
mod window;

use mixtape_session::{Session, account, background, commands, discord, lastfm, net};
use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_window_state::StateFlags;

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

/// The session could not open (e.g. a newer Mixtape moved the library on): say why in a box
/// and leave, instead of the panic that `Builder::run` would turn this into.
fn startup_failed(e: Box<dyn std::error::Error>) -> ! {
    use mixtape_core::l10n::{Lang, set_lang};
    set_lang(Lang::from_locale(mixtape_win::ui_language()));
    let text = match e.downcast::<mixtape_library::Error>() {
        Ok(lib) => mixtape_core::AppError::from(*lib).to_string(),
        Err(e) => e.to_string(),
    };
    tracing::error!("startup failed: {text}");
    mixtape_win::error_box("Mixtape", &text);
    std::process::exit(1)
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
        // A second launch brings the running app's window forward (also out of the tray).
        // A link in its arguments (the browser extension, the protocol handler) is opened too.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            window::show_main(app);
            links::hand_over_all(app, links::links_in(args.into_iter()));
        }))
        // The window is shown by `app_ready` (or stays in the tray), never by the plugin.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all() - StateFlags::VISIBLE)
                // Placed at the cursor on every open.
                .with_denylist(&[tray_menu::LABEL])
                .build(),
        )
        .plugin(window::frame_plugin())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // The session's commands and `cover:`; the session itself opens in `setup`.
        .plugin(mixtape_session::plugin(Arc::new(host::AppHost)))
        .setup(|app| {
            let state = match mixtape_session::open(app.handle()) {
                Ok(state) => state,
                Err(e) => startup_failed(e),
            };
            let prefs = commands::settings::Prefs::load(&state);
            prefs.apply_language(&state);
            account::restore(&state);
            discord::install(app.handle(), prefs.discord_presence);
            lastfm::install(&state);
            net::spawn(state.clone());
            commands::sync::spawn_auto_sync(state.clone());
            // The phone sync server, when a phone is paired.
            commands::peer::spawn(state.clone());
            // Pending likes, artist pictures, unused covers: once, after start.
            background::spawn(state.clone());
            updates::spawn(state.clone());
            updates::spawn_ytdlp(state.clone());
            prefs.apply_playback(&state);
            tracing::debug!("setup: crossfade applied");
            // `setup` runs on the main (UI) thread, which the thumbnail toolbar needs.
            state.media.install(app.handle());
            tracing::debug!("setup: media installed");
            // A start that carries a link shows the window, also when autostart said `--hidden`.
            let start_links = links::links_in(std::env::args().skip(1));
            window::install(app.handle(), autostart::launched_hidden() && start_links.is_empty());
            if let Err(e) = tray::install(app.handle(), &state) {
                tracing::warn!(error = %e, "tray icon unavailable");
            }
            autostart::refresh();
            // The hand-off channel with the island's music module: serve `-app`, claim what
            // the island plays.
            mixtape_session::handoff::app::start(&state);
            // Links from this start and from a second launch that came before the session
            // was up wait for the interface (`app_ready`).
            links::hand_over_all(app.handle(), start_links);
            links::hand_over_early(&state);
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
            if matches!(event, tauri::WindowEvent::Focused(true)) && window.label() == window::MAIN {
                background::nudge_likes();
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == window::MAIN && window::close_to_tray(window.app_handle()) {
                    api.prevent_close();
                } else if window.label() == window::MAIN
                    // Only the main window closes the app; the login and tray windows do not.
                    && let Some(p) = window.app_handle().try_state::<Arc<Session>>().and_then(|st| st.running_player())
                {
                    p.save_snapshot(true);
                    p.finish_listens();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            window::app_ready,
            mini::window_set_mini,
            mini::window_is_mini,
            tray_menu::tray_menu_snapshot,
            tray_menu::tray_menu_reveal,
            tray_menu::tray_menu_hide,
            tray_menu::tray_menu_action,
            autostart::autostart_get,
            autostart::autostart_set,
            updates::app_version,
            updates::update_check,
            updates::update_install,
            updates::ytdlp_check,
            updates::ytdlp_status,
            updates::ytdlp_revert,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Mixtape")
        .run(|app, event| {
            // After the window-state plugin has written its file.
            if let tauri::RunEvent::Exit = event {
                // Quitting (not to the tray): the island may take over from here.
                mixtape_session::handoff::app::bye(app);
                mini::on_exit(app);
                window::on_exit(app);
            }
        });
}
