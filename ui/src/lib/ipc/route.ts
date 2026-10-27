// Where a command lives. The session's commands are the `mixtape` plugin's
// (`plugin:mixtape|<name>`, crates/session); the app's own — window, mini player, tray,
// autostart, updates — exist only in the Mixtape app, not in the island that also runs
// this interface, so they are called only when the host has them (`hostHas`).

export const PLUGIN = "plugin:mixtape|";

/** The app's own commands (src-tauri/build.rs); every other command is the plugin's. */
export const APP_COMMANDS: ReadonlySet<string> = new Set([
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
]);

/** The name to invoke: app commands as they are, the rest with the plugin prefix. */
export function route(cmd: string): string {
  if (cmd.startsWith("plugin:") || APP_COMMANDS.has(cmd)) return cmd;
  return PLUGIN + cmd;
}

/** The bare command name (dev mocks switch on it). */
export function bare(cmd: string): string {
  return cmd.startsWith(PLUGIN) ? cmd.slice(PLUGIN.length) : cmd;
}
