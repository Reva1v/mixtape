// State of the «Трек по ссылке» card in the search (LinkCard.svelte; a file of its own because
// a case-insensitive disk can't hold LinkCard.svelte and linkCard.svelte.ts). A pure class
// with its IPC passed in, so the order of events is testable: the card loads the track row
// (`link_track`) in the background, and every button waits for that before it hands the id
// to the player — the player can only take ids that already have a row.

import type { SharedLink } from "$lib/ipc/bindings/SharedLink";
import type { Track } from "$lib/ipc/bindings/Track";

export type LinkPhase =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; track: Track }
  | { status: "failed"; message: string };

/** The three buttons; Enter in the search box is «now». */
export type LinkAction = "now" | "next" | "end";

export interface LinkIo {
  loadTrack(id: string): Promise<Track>;
  play(id: string): Promise<unknown>;
  queueAdd(id: string, position: "next" | "end"): Promise<unknown>;
  /** A queue action went through (the screen shows its toast). */
  done(action: LinkAction): void;
  /** An action failed even with the bare id. */
  failed(error: unknown): void;
}

const same = (a: SharedLink | null, b: SharedLink | null) => a?.kind === b?.kind && a?.id === b?.id;

export class LinkCardState {
  link = $state.raw<SharedLink | null>(null);
  phase = $state.raw<LinkPhase>({ status: "idle" });
  /** The action waiting for the track or running; the buttons show a spinner and lock. */
  busy = $state<LinkAction | null>(null);

  /** Bumped per link: an answer for an older link finds a newer number and is dropped. */
  #gen = 0;
  /** The current `link_track` call, settled either way (never rejects). */
  #settled: Promise<void> = Promise.resolve();

  constructor(private io: LinkIo) {}

  /** Shows `link` (null: no card). The same link again changes nothing. */
  set(link: SharedLink | null) {
    if (same(link, this.link)) return;
    const gen = ++this.#gen;
    this.link = link;
    this.busy = null;
    if (link?.kind !== "track") {
      this.phase = { status: "idle" };
      this.#settled = Promise.resolve();
      return;
    }
    this.phase = { status: "loading" };
    this.#settled = this.io.loadTrack(link.id).then(
      (track) => {
        if (gen === this.#gen) this.phase = { status: "ready", track };
      },
      (e) => {
        if (gen === this.#gen)
          this.phase = { status: "failed", message: e instanceof Error ? e.message : String(e) };
      },
    );
  }

  /** Runs `action` for the track. Waits for the loading first; if the loading failed, the
   * action is still tried once with the bare id (the row may exist from before, e.g. the
   * track is in the library). A second press while one is running does nothing. Resolves
   * `true` when the action went through; a failure goes to `io.failed`. */
  async act(action: LinkAction): Promise<boolean> {
    const link = this.link;
    if (link?.kind !== "track" || this.busy) return false;
    const gen = this.#gen;
    const settled = this.#settled;
    this.busy = action;
    try {
      await settled;
      if (action === "now") await this.io.play(link.id);
      else await this.io.queueAdd(link.id, action);
      this.io.done(action);
      return true;
    } catch (e) {
      this.io.failed(e);
      return false;
    } finally {
      if (gen === this.#gen) this.busy = null;
    }
  }

  /** Enter in the search box. */
  play() {
    return this.act("now");
  }
}
