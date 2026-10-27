import { mount } from "svelte";
import { isTauri } from "@tauri-apps/api/core";
import { invoke } from "$lib/ipc";
import "./app.css";
import App from "./App.svelte";
import { installSpringCss, warmUpCompositor } from "$lib/motion";
import { prefs } from "$lib/stores/prefs.svelte";

installSpringCss();

// Dev only: `?mock` fakes the sync/downloads commands in a plain browser (lib/ipc/mock.ts).
if (import.meta.env.DEV && !isTauri() && new URLSearchParams(location.search).has("mock")) await import("$lib/ipc/mock");

// The window moves and maximizes (double-click) only from the top edge, like a native
// title bar. Empty areas lower down are drag regions too (panel gaps, sidebar, the
// full-screen player), so Tauri's handler is stopped for presses below the bar.
const TITLE_BAR_H = 44;
window.addEventListener(
  "mousedown",
  (e) => {
    const el = e.target as HTMLElement | null;
    // The mini player moves by any empty spot, but a double click must not maximize it.
    if (document.documentElement.classList.contains("mini")) {
      if (e.detail >= 2 && !el?.closest?.("button")) e.stopImmediatePropagation();
      return;
    }
    if (e.clientY > TITLE_BAR_H && el?.hasAttribute?.("data-tauri-drag-region")) {
      e.stopImmediatePropagation();
    }
  },
  true,
);

// Preferences carry the interface language: read them before the first frame, so the
// window never shows in one language and then switches.
await prefs.init().catch((e) => console.error("prefs init failed", e));

const app = mount(App, { target: document.getElementById("app")! });

// Show the window only after the first frame is painted, so there is no white flash.
// The island runs this interface in a window of its own and has no `app_ready` (hostHas).
requestAnimationFrame(() => requestAnimationFrame(() => void invoke("app_ready").catch((e) => console.error("app_ready", e))));
warmUpCompositor();

export default app;

// The webview's own context menu (save image, inspect…) is not part of the app. Custom
// menus call preventDefault themselves; everything else is blocked here, except in text
// fields, where cut/copy/paste is useful. Shift+right-click keeps it in dev builds.
window.addEventListener("contextmenu", (e) => {
  if (e.defaultPrevented) return;
  if (import.meta.env.DEV && e.shiftKey) return;
  const el = e.target as HTMLElement | null;
  if (el?.closest("input, textarea, [contenteditable=true]")) return;
  e.preventDefault();
});
