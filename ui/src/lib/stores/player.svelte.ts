// Player state mirrored from the Rust core. Updates are applied field by field, so a
// position update only wakes what reads the position; `playing` and `currentId` change
// only when the status or the track does. Position arrives now and then (and on every
// status/track/seek change); progress UI extrapolates it with `livePosition()` and
// redraws through `followProgress()`, a few times a second rather than every frame.

import { untrack } from "svelte";
import { Channel, invoke, on } from "$lib/ipc";
import { toasts } from "./overlay.svelte";
import { prefs } from "./prefs.svelte";
import { layout } from "./layout.svelte";
import type { Track } from "$lib/ipc/bindings/Track";
import type { Repeat } from "$lib/ipc/bindings/Repeat";
import type { PlaybackState } from "$lib/ipc/bindings/PlaybackState";
import type { TrackChanged } from "$lib/ipc/bindings/TrackChanged";
import type { PlaySource } from "$lib/ipc/bindings/PlaySource";

class Player {
  state = $state<PlaybackState>({
    status: "idle",
    positionMs: 0,
    durationMs: 0,
    volume: 0.8,
    repeat: "off",
    shuffle: false,
    timestampMs: 0,
  });
  track = $state.raw<Track | null>(null);
  palette = $state.raw<string[]>([]);
  /** Bumped when the queue changes so panels refetch. */
  queueVersion = $state(0);

  playing = $derived(this.state.status === "playing");
  currentId = $derived(this.state.trackId ?? null);

  async init() {
    const channel = new Channel<PlaybackState>();
    channel.onmessage = (s) => {
      this.#receivedAt = performance.now();
      // Only fields whose value changed notify their readers.
      const st = this.state;
      st.trackId = s.trackId;
      st.status = s.status;
      st.positionMs = s.positionMs;
      st.durationMs = s.durationMs;
      st.volume = s.volume;
      st.repeat = s.repeat;
      st.shuffle = s.shuffle;
      st.timestampMs = s.timestampMs;
    };
    await on<TrackChanged>("track:changed", (e) => {
      this.track = e.track;
      this.palette = e.palette;
      applyAccent(e.palette);
    });
    await on("queue:changed", () => this.queueVersion++);
    // A like, a finished download or «Скачать» changes the playing track's row: refresh
    // the copy the players and menus show (the palette stays).
    await on("library:changed", () => void this.refreshTrack());
    // The mix ran out and YouTube Music is out of reach.
    await on("mix:offline", () =>
      toasts.push("Нет связи с YouTube Music — микс не продолжается", "error", 12000, {
        label: "Продолжить из библиотеки",
        run: () => void invoke("mix_continue_local").catch(() => {}),
      }),
    );
    await invoke("player_subscribe", { channel });
  }

  #receivedAt = 0;

  /** Position extrapolated from the last update while playing. */
  livePosition(): number {
    const s = this.state;
    if (s.status !== "playing") return s.positionMs;
    const ms = s.positionMs + (performance.now() - this.#receivedAt);
    return Math.min(ms, s.durationMs || ms);
  }

  play(source: PlaySource, startIndex = 0) {
    return invoke("play", { source, startIndex });
  }
  toggle() {
    return invoke("player_toggle");
  }
  next() {
    return invoke("player_next");
  }
  prev() {
    return invoke("player_prev");
  }
  seek(positionMs: number) {
    this.state.positionMs = positionMs;
    this.#receivedAt = performance.now();
    return invoke("player_seek", { positionMs: Math.round(positionMs) });
  }
  setVolume(volume: number) {
    this.state.volume = volume;
    return invoke("player_set_volume", { volume });
  }
  setRepeat(repeat: Repeat) {
    this.state.repeat = repeat;
    return invoke("player_set_repeat", { repeat });
  }
  cycleRepeat() {
    const order: Repeat[] = ["off", "all", "one"];
    return this.setRepeat(order[(order.indexOf(this.state.repeat) + 1) % order.length]);
  }
  toggleShuffle() {
    this.state.shuffle = !this.state.shuffle;
    return invoke("player_set_shuffle", { on: this.state.shuffle });
  }
  queueAdd(trackIds: string[], position: "next" | "end") {
    return invoke("queue_add", { trackIds, position });
  }
  async refreshTrack() {
    const id = this.track?.id;
    if (!id) return;
    const fresh = await invoke<Track | null>("track_get", { id }).catch(() => null);
    if (fresh && this.track?.id === id) this.track = fresh;
  }

  /** Replaces the queue with a mix built from this track (US3). */
  mix(trackId: string) {
    return this.startMix(() => invoke("mix_start", { trackId }));
  }

  /** Every way of starting a mix goes through here. With «Включать полноэкранный режим»
   * on, the full-screen player opens at once, not after the mix is fetched; if the mix
   * fails, it closes again (only if it was opened here). */
  async startMix(start: () => Promise<unknown>) {
    toasts.push("Собираю микс…");
    const opened = prefs.value.mixFullscreen && !layout.fullscreen;
    if (opened) layout.fullscreen = true;
    try {
      await this.#building(start);
    } catch (e) {
      if (opened) layout.fullscreen = false;
      toasts.push(e instanceof Error && e.message ? e.message : "Не удалось включить микс", "error");
    }
  }

  /** Switches the mix to another chip; the queue shows the building state meanwhile. */
  selectChip(chipId: string) {
    return this.#building(() => invoke("mix_select_chip", { chipId })).catch(() => {});
  }

  /** A mix is being built (started or switched to another chip): the queue shows the
   * playing track and placeholder rows until the new tracks arrive. */
  mixPending = $state(false);
  #pendingCount = 0;

  async #building<T>(run: () => Promise<T>): Promise<T> {
    this.#pendingCount++;
    this.mixPending = true;
    try {
      return await run();
    } finally {
      if (--this.#pendingCount === 0) this.mixPending = false;
    }
  }
}

/** Sets `--glow`, the artwork tint behind headers and the full-screen player. */
function applyAccent(palette: string[]) {
  document.documentElement.style.setProperty("--glow", glowOf(palette));
}

/** Picks the most saturated cover colour and darkens it into a background tint. */
export function glowOf(palette: string[]): string {
  let best = { hex: "", score: -1 };
  for (const hex of palette) {
    const n = parseInt(hex.slice(1), 16);
    const r = (n >> 16) / 255, g = ((n >> 8) & 255) / 255, b = (n & 255) / 255;
    const max = Math.max(r, g, b), min = Math.min(r, g, b);
    const l = (max + min) / 2;
    const s = max === min ? 0 : (max - min) / (1 - Math.abs(2 * l - 1));
    const score = s * (l > 0.12 && l < 0.9 ? 1 : 0.2);
    if (score > best.score) best = { hex, score };
  }
  if (!best.hex || best.score < 0.12) return "#2a2a2e";
  const n = parseInt(best.hex.slice(1), 16);
  const k = 0.45;
  const c = (v: number) => Math.round(v * k).toString(16).padStart(2, "0");
  return `#${c(n >> 16)}${c((n >> 8) & 255)}${c(n & 255)}`;
}

export const player = new Player();

/**
 * Keeps a progress display current without a per-frame loop. Call it inside `$effect`
 * and return its result: `draw` gets the live position now and, while playing and
 * `live`, again whenever the position has moved about one device pixel along a bar
 * `widthPx` wide (at most every 50 ms, at least every `maxStepMs`). That looks as smooth
 * as redrawing every frame, but wakes the page a few times a second instead of 60+.
 * Re-runs (and redraws at once) when the position, status or duration changes.
 */
export function followProgress(widthPx: number, draw: (ms: number) => void, live = true, maxStepMs = 1000): () => void {
  const { status, durationMs } = player.state;
  void player.state.positionMs;
  untrack(() => draw(player.livePosition()));
  if (status !== "playing" || !live || durationMs <= 0) return () => {};
  const px = Math.max(1, widthPx * (window.devicePixelRatio || 1));
  const step = Math.min(maxStepMs, Math.max(50, durationMs / px));
  let id = 0;
  const tick = () => {
    draw(player.livePosition());
    id = window.setTimeout(tick, step);
  };
  id = window.setTimeout(tick, step);
  return () => clearTimeout(id);
}

export function formatTime(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}
