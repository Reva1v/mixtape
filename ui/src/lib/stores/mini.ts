// Mini player: the core shrinks the window into a small always-on-top strip and says so
// with `window:mini`; the page then shows `MiniWindow` instead of the full layout. The
// bottom bar button, Ctrl+M and the tray menu all go through the core, so they agree.

import { invoke, on } from "$lib/ipc";
import { layout } from "./layout.svelte";

function apply(active: boolean) {
  if (active) layout.fullscreen = false;
  layout.mini = active;
  document.documentElement.classList.toggle("mini", active);
}

export const mini = {
  async init() {
    await on<boolean>("window:mini", apply);
    // After a page reload the window may already be the strip.
    if (await invoke<boolean>("window_is_mini").catch(() => false)) apply(true);
  },
  set(on: boolean) {
    return invoke("window_set_mini", { on }).catch((e) => console.error("mini player", e));
  },
  toggle() {
    return this.set(!layout.mini);
  },
};
