// Global keyboard shortcuts (FR-025). Keys are matched by `e.code`, so they work in any
// keyboard layout. Ignored while typing in a field. The help dialog lists `shortcuts`.

import { player } from "./player.svelte";
import { layout } from "./layout.svelte";
import { router } from "./router.svelte";
import { downloads } from "./downloads.svelte";
import { mini } from "./mini";
import { setLiked } from "$lib/ui/trackMenu";
import { t } from "$lib/i18n";

let muteRestore = 0.8;

function typing(e: KeyboardEvent) {
  const el = e.target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

/** Space on a focused control belongs to it (press a button, toggle a switch), not to play/pause. */
function ownsSpace(e: KeyboardEvent) {
  const el = e.target as HTMLElement | null;
  return !!el?.closest?.("button, a[href], summary, [role=button], [role=slider], [role=switch], [role=checkbox], [role=tab], [role=menuitem], [role=option]");
}

function focusSearch() {
  if (router.current.name !== "search") router.go({ name: "search" });
  layout.searchFocus++;
}

// Clamped like the seek bar: never to the very end, which would skip to the next track.
const seekBy = (ms: number) => void player.scrubTo(player.livePosition() + ms);
const volumeBy = (d: number) =>
  void player.setVolume(Math.round(Math.min(1, Math.max(0, player.state.volume + d)) * 100) / 100);

function toggleMute() {
  if (player.state.volume > 0) {
    muteRestore = player.state.volume;
    void player.setVolume(0);
  } else void player.setVolume(muteRestore || 0.8);
}

/** Shown in the shortcuts dialog, grouped. A function, so it is worded in the current language. */
export function shortcuts(): { group: string; items: [keys: string, action: string][] }[] {
  const h = t.hotkeys;
  return [
    {
      group: h.playback,
      items: [
        [`${h.space}  ·  K`, h.playPause],
        ["Ctrl + →  ·  Shift + →", h.nextTrack],
        ["Ctrl + ←  ·  Shift + ←", h.prevTrack],
        ["→  /  ←", h.seek5],
        ["J  /  ;", h.seek10],
        ["0 … 9", h.seekPercent],
        ["S", t.player.shuffle],
        ["R", h.repeat],
        ["L", h.like],
      ],
    },
    {
      group: t.player.volume,
      items: [
        ["↑  /  ↓", h.volume],
        ["M", t.player.mute],
      ],
    },
    {
      group: h.interface,
      items: [
        ["F", t.player.fullscreen],
        ["Ctrl + M", t.tray.miniPlayer],
        ["Q", t.player.queue],
        ["T", h.lyrics],
        ["Ctrl + K  ·  /", t.nav.search],
        ["Ctrl + 1 … 4", h.tabs],
        ["Ctrl + ,", t.nav.settings],
        ["Ctrl + L", t.importUrl.title],
        ["Ctrl + V", h.pasteLink],
        ["Alt + ←  /  Alt + →", h.backForward],
        ["Esc", h.escape],
        ["?", h.help],
      ],
    },
  ];
}

/** Keys for the full layout, which the mini player does not show. */
function fullLayoutOnly(e: KeyboardEvent, ctrl: boolean) {
  if (e.altKey) return true;
  if (ctrl) return !e.code.startsWith("Arrow");
  return ["KeyQ", "KeyT", "KeyF", "Slash", "Escape"].includes(e.code) || e.key === "?";
}

export function handleHotkey(e: KeyboardEvent): boolean {
  const ctrl = e.ctrlKey || e.metaKey;

  if (ctrl && !e.shiftKey && !e.altKey && e.code === "KeyM") return (void mini.toggle(), true);
  if (layout.mini && fullLayoutOnly(e, ctrl)) return false;

  // Work even while typing.
  if (ctrl && e.code === "KeyK") return (focusSearch(), true);
  if (ctrl && e.code === "Comma") return (router.go({ name: "settings" }), true);
  if (ctrl && e.code === "KeyL") return ((downloads.importUrl ??= ""), true);
  if (ctrl && !e.shiftKey && !e.altKey) {
    const tabs = { Digit1: "home", Digit2: "search", Digit3: "library", Digit4: "downloads" } as const;
    const tab = tabs[e.code as keyof typeof tabs];
    if (tab) {
      router.go(tab === "library" ? { name: "library", tab: "tracks" } : { name: tab });
      return true;
    }
  }
  if (typing(e)) return false;

  if (e.altKey) {
    if (e.code === "ArrowLeft") return (router.back(), true);
    if (e.code === "ArrowRight") return (router.forward(), true);
    return false;
  }

  // Track switching: Ctrl or Shift with arrows.
  if ((ctrl || e.shiftKey) && e.code === "ArrowRight") return (void player.next(), true);
  if ((ctrl || e.shiftKey) && e.code === "ArrowLeft") return (void player.prev(), true);
  if (ctrl && e.code === "ArrowUp") return (volumeBy(0.05), true);
  if (ctrl && e.code === "ArrowDown") return (volumeBy(-0.05), true);
  if (ctrl) return false;

  if (e.key === "?" || (e.shiftKey && e.code === "Slash")) return ((layout.shortcutsOpen = !layout.shortcutsOpen), true);
  if (e.key === "/") return (focusSearch(), true);

  if (/^Digit\d$/.test(e.code) && player.state.durationMs) {
    const d = Number(e.code.slice(5));
    void player.seek((player.state.durationMs * d) / 10);
    return true;
  }

  switch (e.code) {
    case "Space":
      if (ownsSpace(e)) return false;
      void player.toggle();
      return true;
    case "KeyK":
      void player.toggle();
      return true;
    case "ArrowRight":
      seekBy(5000);
      return true;
    case "ArrowLeft":
      seekBy(-5000);
      return true;
    case "Semicolon":
      seekBy(10000);
      return true;
    case "KeyJ":
      seekBy(-10000);
      return true;
    case "ArrowUp":
      volumeBy(0.05);
      return true;
    case "ArrowDown":
      volumeBy(-0.05);
      return true;
    case "KeyM":
      toggleMute();
      return true;
    case "KeyS":
      void player.toggleShuffle();
      return true;
    case "KeyR":
      void player.cycleRepeat();
      return true;
    case "KeyL":
      if (player.track) void setLiked(player.track, !player.track.liked);
      return true;
    case "KeyQ":
      layout.toggleDrawer("queue");
      return true;
    case "KeyT":
      if (player.track) layout.toggleLyrics();
      return true;
    case "KeyF":
      if (player.track) layout.fullscreen = !layout.fullscreen;
      return true;
    case "Escape":
      if (layout.shortcutsOpen) return ((layout.shortcutsOpen = false), true);
      if (layout.drawer) return ((layout.drawer = null), true);
      return false;
  }
  return false;
}
