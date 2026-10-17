import { mount } from "svelte";
import { invoke, isTauri } from "@tauri-apps/api/core";
import "./app.css";
import App from "./App.svelte";
import { installSpringCss } from "$lib/motion";

installSpringCss();

// Dev only: `?mock` fakes the sync/downloads commands in a plain browser (lib/ipc/mock.ts).
if (import.meta.env.DEV && !isTauri() && new URLSearchParams(location.search).has("mock")) await import("$lib/ipc/mock");

// Double-click maximizes only along the top edge, like a native title bar. Elsewhere
// drag regions only move the window: stop Tauri's handler for the second click there.
const TITLE_BAR_H = 44;
window.addEventListener(
  "mousedown",
  (e) => {
    const el = e.target as HTMLElement | null;
    if (e.detail === 2 && e.clientY > TITLE_BAR_H && el?.hasAttribute?.("data-tauri-drag-region")) {
      e.stopImmediatePropagation();
    }
  },
  true,
);

const app = mount(App, { target: document.getElementById("app")! });

// Show the window only after the first frame is painted, so there is no white flash.
requestAnimationFrame(() => requestAnimationFrame(() => void invoke("app_ready")));

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
