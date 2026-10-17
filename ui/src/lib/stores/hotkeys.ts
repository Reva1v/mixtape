// Global keyboard shortcuts (FR-025). Ignored while typing in an input.

import { player } from "./player.svelte";
import { layout } from "./layout.svelte";
import { router } from "./router.svelte";

let muteRestore = 0.8;

function typing(e: KeyboardEvent) {
  const el = e.target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

function focusSearch() {
  if (router.current.name !== "search") router.go({ name: "search" });
  layout.searchFocus++;
}

export function handleHotkey(e: KeyboardEvent): boolean {
  if ((e.ctrlKey && e.code === "KeyK") || (e.key === "/" && !typing(e))) {
    focusSearch();
    return true;
  }
  if (e.altKey && e.key === "ArrowLeft") return (router.back(), true);
  if (e.altKey && e.key === "ArrowRight") return (router.forward(), true);
  if (typing(e) || e.ctrlKey || e.altKey || e.metaKey) return false;

  const vol = player.state.volume;
  switch (e.code) {
    case "Space":
      void player.toggle();
      return true;
    case "ArrowRight":
      if (e.shiftKey) void player.next();
      else void player.seek(Math.min(player.livePosition() + 5000, player.state.durationMs));
      return true;
    case "ArrowLeft":
      if (e.shiftKey) void player.prev();
      else void player.seek(Math.max(player.livePosition() - 5000, 0));
      return true;
    case "ArrowUp":
      void player.setVolume(Math.min(1, vol + 0.05));
      return true;
    case "ArrowDown":
      void player.setVolume(Math.max(0, vol - 0.05));
      return true;
    case "KeyM":
      if (vol > 0) {
        muteRestore = vol;
        void player.setVolume(0);
      } else void player.setVolume(muteRestore);
      return true;
    case "KeyQ":
      layout.toggleDrawer("queue");
      return true;
    case "KeyL":
      layout.toggleDrawer("lyrics");
      return true;
    case "KeyF":
      if (player.track) layout.fullscreen = !layout.fullscreen;
      return true;
    case "Escape":
      if (layout.drawer) {
        layout.drawer = null;
        return true;
      }
      return false;
  }
  return false;
}
