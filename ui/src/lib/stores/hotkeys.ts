// Global keyboard shortcuts (FR-025). Keys are matched by `e.code`, so they work in any
// keyboard layout. Ignored while typing in a field. The help dialog lists `shortcuts`.

import { player } from "./player.svelte";
import { layout } from "./layout.svelte";
import { router } from "./router.svelte";
import { setLiked } from "$lib/ui/trackMenu";

let muteRestore = 0.8;

function typing(e: KeyboardEvent) {
  const el = e.target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

function focusSearch() {
  if (router.current.name !== "search") router.go({ name: "search" });
  layout.searchFocus++;
}

const seekBy = (ms: number) =>
  void player.seek(Math.min(Math.max(player.livePosition() + ms, 0), player.state.durationMs));
const volumeBy = (d: number) =>
  void player.setVolume(Math.round(Math.min(1, Math.max(0, player.state.volume + d)) * 100) / 100);

function toggleMute() {
  if (player.state.volume > 0) {
    muteRestore = player.state.volume;
    void player.setVolume(0);
  } else void player.setVolume(muteRestore || 0.8);
}

/** Shown in the shortcuts dialog, grouped. */
export const shortcuts: { group: string; items: [keys: string, action: string][] }[] = [
  {
    group: "Воспроизведение",
    items: [
      ["Пробел  ·  K", "Играть / пауза"],
      ["Ctrl + →  ·  Shift + →", "Следующий трек"],
      ["Ctrl + ←  ·  Shift + ←", "Предыдущий трек"],
      ["→  /  ←", "Перемотка на 5 секунд"],
      ["J  /  ;", "Перемотка на 10 секунд"],
      ["0 … 9", "Перейти к 0 % … 90 % трека"],
      ["S", "Перемешать"],
      ["R", "Повтор: выкл → всё → трек"],
      ["L", "Нравится / убрать"],
    ],
  },
  {
    group: "Громкость",
    items: [
      ["↑  /  ↓", "Громче / тише на 5 %"],
      ["M", "Без звука"],
    ],
  },
  {
    group: "Интерфейс",
    items: [
      ["F", "Полноэкранный плеер"],
      ["Q", "Очередь"],
      ["T", "Текст песни"],
      ["Ctrl + K  ·  /", "Поиск"],
      ["Ctrl + 1 … 4", "Главная, поиск, библиотека, загрузки"],
      ["Ctrl + ,", "Настройки"],
      ["Alt + ←  /  Alt + →", "Назад / вперёд"],
      ["Esc", "Закрыть панель или полноэкранный режим"],
      ["?", "Эта подсказка"],
    ],
  },
];

export function handleHotkey(e: KeyboardEvent): boolean {
  const ctrl = e.ctrlKey || e.metaKey;

  // Work even while typing.
  if (ctrl && e.code === "KeyK") return (focusSearch(), true);
  if (ctrl && e.code === "Comma") return (router.go({ name: "settings" }), true);
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
      layout.toggleDrawer("lyrics");
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
