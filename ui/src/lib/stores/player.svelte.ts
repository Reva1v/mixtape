// Player state mirrored from the Rust core. Position arrives at ≤10 Hz; components
// that draw progress call `livePosition()` inside requestAnimationFrame instead of
// re-rendering on every tick.

import { Channel, invoke, on } from "$lib/ipc";
import type { Track } from "$lib/ipc/bindings/Track";
import type { Repeat } from "$lib/ipc/bindings/Repeat";
import type { PlaybackState } from "$lib/ipc/bindings/PlaybackState";
import type { TrackChanged } from "$lib/ipc/bindings/TrackChanged";
import type { PlaySource } from "$lib/ipc/bindings/PlaySource";

const DEFAULT_ACCENT = ["#ff4d8d", "#7b5cff"];

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

  get playing() {
    return this.state.status === "playing";
  }
  get currentId() {
    return this.state.trackId ?? null;
  }

  async init() {
    const channel = new Channel<PlaybackState>();
    channel.onmessage = (s) => {
      this.state = s;
      this.#receivedAt = performance.now();
    };
    await on<TrackChanged>("track:changed", (e) => {
      this.track = e.track;
      this.palette = e.palette;
      applyAccent(e.palette);
    });
    await on("queue:changed", () => this.queueVersion++);
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
  mix(trackId: string) {
    return invoke("mix_start", { trackId });
  }
}

function applyAccent(palette: string[]) {
  const [a, b] = palette.length >= 2 ? pickAccents(palette) : DEFAULT_ACCENT;
  const root = document.documentElement.style;
  root.setProperty("--accent", a);
  root.setProperty("--accent-2", b);
}

/** Prefers the most saturated, not-too-dark colours of the cover for the accent. */
function pickAccents(palette: string[]): [string, string] {
  const scored = palette
    .map((hex) => {
      const n = parseInt(hex.slice(1), 16);
      const r = (n >> 16) / 255, g = ((n >> 8) & 255) / 255, b = (n & 255) / 255;
      const max = Math.max(r, g, b), min = Math.min(r, g, b);
      const l = (max + min) / 2;
      const s = max === min ? 0 : (max - min) / (1 - Math.abs(2 * l - 1));
      return { hex, score: s * (l > 0.25 && l < 0.85 ? 1 : 0.3) };
    })
    .sort((x, y) => y.score - x.score);
  if (scored[0].score < 0.15) return [DEFAULT_ACCENT[0], DEFAULT_ACCENT[1]];
  return [scored[0].hex, scored[1]?.hex ?? scored[0].hex];
}

export const player = new Player();

export function formatTime(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}
