import { mount } from "svelte";
import { invoke } from "@tauri-apps/api/core";
import "./app.css";
import App from "./App.svelte";
import { installSpringCss } from "$lib/motion";

installSpringCss();

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
