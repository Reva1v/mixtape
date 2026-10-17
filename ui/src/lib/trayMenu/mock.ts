// Dev-only fake core for the tray menu page in a plain browser: `tray-menu.html?mock`.
// Switches: `lang=en|uk|ru`, `idle` (nothing playing), `paused`, `shown` (the main window
// is on the screen), `mini`. The track and its cover are made up.
import { mockIPC } from "@tauri-apps/api/mocks";
import { setCoverSource } from "$lib/ipc";
import type { Lang } from "$lib/ipc/bindings/Lang";
import type { Track } from "$lib/ipc/bindings/Track";
import type { TrayMenuSnapshot } from "$lib/ipc/bindings/TrayMenuSnapshot";

const params = new URLSearchParams(location.search);
const lang = (["en", "uk", "ru"].includes(params.get("lang") ?? "") ? params.get("lang") : "en") as Lang;

const track: Track = {
  id: "mock-1",
  title: "Neon Harbour",
  artists: [{ id: "mock-a", name: "Velvet Static" }],
  durationMs: 214000,
  coverKey: "mock",
  state: "downloaded",
  liked: true,
};

setCoverSource(() => {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ff6b6b"/><stop offset="0.55" stop-color="#5f27cd"/><stop offset="1" stop-color="#1e1e3f"/></linearGradient></defs><rect width="64" height="64" fill="url(#g)"/><circle cx="44" cy="20" r="10" fill="#feca57" opacity="0.85"/></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
});

const snapshot: TrayMenuSnapshot = {
  lang,
  track: params.has("idle") ? null : track,
  playing: !params.has("paused") && !params.has("idle"),
  windowShown: params.has("shown"),
  mini: params.has("mini"),
};

mockIPC(
  (cmd) => {
    switch (cmd) {
      case "tray_menu_snapshot":
        return snapshot;
      case "tray_menu_reveal":
      case "tray_menu_hide":
      case "tray_menu_action":
      case "player_toggle":
      case "player_next":
      case "player_prev":
      case "track_set_liked":
        return null;
      default:
        throw new Error(`mock: unknown command ${cmd}`);
    }
  },
  { shouldMockEvents: true },
);
