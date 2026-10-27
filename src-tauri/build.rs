/// Every command of the app itself in `generate_handler!` (`src/lib.rs`): the window, mini
/// player, tray menu, autostart and updates; the rest are the `mixtape` plugin's
/// (`crates/session/build.rs`). Listing them gives each an `allow-<command>` permission and
/// makes the ACL deny any command a window's capability (`capabilities/*.json`) does not
/// grant: the tray menu window can call only the few it needs. A new command goes here and
/// into a capability, or the UI gets "not allowed".
const COMMANDS: &[&str] = &[
    "app_ready",
    "window_set_mini",
    "window_is_mini",
    "tray_menu_snapshot",
    "tray_menu_reveal",
    "tray_menu_hide",
    "tray_menu_action",
    "autostart_get",
    "autostart_set",
    "app_version",
    "update_check",
    "update_install",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
