use std::path::Path;

/// Every command of the plugin (`generate_handler!` in `src/lib.rs`). Each gets an
/// `allow-<command>` permission, granted to an app's windows as `mixtape:allow-<command>`
/// in its capabilities; a command missing here or there is "not allowed" in the interface.
const COMMANDS: &[&str] = &[
    "host_caps",
    "library_view",
    "library_rows",
    "library_search",
    "track_get",
    "album_get",
    "artist_get",
    "playlist_get",
    "playlist_edit",
    "catalog_search",
    "catalog_more",
    "catalog_suggest",
    "artist_catalog",
    "artist_songs",
    "catalog_download",
    "track_set_liked",
    "track_download",
    "mix_start",
    "mix_select_chip",
    "mix_continue_local",
    "home_get",
    "home_shelves",
    "mix_start_playlist",
    "player_subscribe",
    "play",
    "player_toggle",
    "player_next",
    "player_prev",
    "player_seek",
    "player_set_volume",
    "player_set_repeat",
    "player_set_shuffle",
    "queue_get",
    "queue_add",
    "queue_remove",
    "queue_move",
    "queue_clear",
    "queue_jump",
    "lyrics_get",
    "stats_get",
    "peer_status",
    "peer_pair_start",
    "peer_pair_cancel",
    "peer_unpair",
    "peer_firewall_add",
    "dev_import_folder",
    "prefs_get",
    "prefs_set",
    "storage_get",
    "storage_set_cache_limit",
    "storage_set_autosave",
    "cache_clear",
    "net_status",
    "open_data_dir",
    "integrations_status",
    "lastfm_connect",
    "lastfm_confirm",
    "lastfm_disconnect",
    "account_status",
    "account_connect_cookies",
    "account_connect_webview",
    "account_disconnect",
    "sync_preview",
    "sync_start",
    "import_url",
    "spotify_import",
    "spotify_cancel",
    "spotify_download",
    "downloads_pause",
    "downloads_resume",
    "download_retry",
    "downloads_failed",
    "downloads_subscribe",
];

fn main() {
    secrets();
    tauri_plugin::Builder::new(COMMANDS).build();
}

/// Last.fm and Discord keys are not in the repository. They come from the environment at
/// compile time (`MIXTAPE_LASTFM_KEY`, `MIXTAPE_LASTFM_SECRET`, `MIXTAPE_DISCORD_APP_ID`)
/// or, when those are unset, from the gitignored `.secrets/lastfm.json`
/// (`{"key": "...", "secret": "..."}`) and `.secrets/discord.json` (`{"appId": "..."}`).
/// Without them the features are hidden.
fn secrets() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.secrets");
    from_file(
        &dir.join("lastfm.json"),
        &[
            ("key", "MIXTAPE_LASTFM_KEY"),
            ("secret", "MIXTAPE_LASTFM_SECRET"),
        ],
    );
    from_file(
        &dir.join("discord.json"),
        &[("appId", "MIXTAPE_DISCORD_APP_ID")],
    );
    // Watch only what exists: a missing path would rerun this script on every build.
    // The folder's listing changes when a file appears in it; without the folder, the
    // workspace root's does when it is created.
    let watch = [
        dir.join("lastfm.json"),
        dir.join("discord.json"),
        dir.clone(),
    ];
    let existing: Vec<_> = watch.iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        println!(
            "cargo:rerun-if-changed={}",
            dir.parent().unwrap_or(&dir).display()
        );
    }
    for p in existing {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}

fn from_file(path: &Path, fields: &[(&str, &str)]) {
    for (_, var) in fields {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        println!("cargo:warning={} is not valid JSON", path.display());
        return;
    };
    for (field, var) in fields {
        if std::env::var_os(var).is_some() {
            continue;
        }
        if let Some(v) = json
            .get(field)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            println!("cargo:rustc-env={var}={v}");
        }
    }
}
