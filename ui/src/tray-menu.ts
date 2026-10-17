// Entry of the tray icon's menu window (src-tauri/src/tray_menu.rs). A page of its own,
// so the menu loads a few kilobytes instead of the whole app.
import { mount } from "svelte";
import { isTauri } from "@tauri-apps/api/core";
import "./tray-menu.css";
import TrayMenu from "$lib/trayMenu/TrayMenu.svelte";

// Dev only: `?mock` fakes the core in a plain browser (lib/trayMenu/mock.ts).
if (import.meta.env.DEV && !isTauri() && new URLSearchParams(location.search).has("mock")) await import("$lib/trayMenu/mock");

// No webview menu (save image, inspect…) over our own.
window.addEventListener("contextmenu", (e) => {
  if (!(import.meta.env.DEV && e.shiftKey)) e.preventDefault();
});

export default mount(TrayMenu, { target: document.getElementById("app")! });
