// A tiny in-memory router: a desktop app has no URL bar, just screens and history.

import { layout } from "./layout.svelte";

export type LibraryTab = "tracks" | "albums" | "artists" | "playlists" | "liked";

export type Route =
  | { name: "home" }
  | { name: "search"; query?: string }
  | { name: "library"; tab: LibraryTab }
  | { name: "album"; id: string }
  | { name: "artist"; id: string }
  | { name: "playlist"; id: string }
  | { name: "downloads" }
  | { name: "settings" };

type Entry = { route: Route; scroll: number };

class Router {
  #back = $state<Entry[]>([]);
  #forward = $state<Entry[]>([]);
  current = $state<Route>({ name: "home" });
  /** Scroll offset to restore after `back`/`forward`; `null` for a new page (top). */
  restoreScroll: number | null = null;
  /** How the current page was reached; picks the page transition. */
  direction: "new" | "back" | "forward" = "new";
  /** Reads the main scroll offset; set by the app shell. */
  scrollSource: () => number = () => 0;

  get canBack() {
    return this.#back.length > 0;
  }
  get canForward() {
    return this.#forward.length > 0;
  }

  #here(): Entry {
    return { route: this.current, scroll: this.scrollSource() };
  }

  go(route: Route) {
    // Pages open underneath the full-screen player, so leave it first.
    layout.fullscreen = false;
    if (JSON.stringify(route) === JSON.stringify(this.current)) return;
    this.#back.push(this.#here());
    if (this.#back.length > 50) this.#back.shift();
    this.#forward = [];
    this.restoreScroll = null;
    this.direction = "new";
    this.current = route;
  }

  /** Same as `go`, but replaces the current entry (tabs, search typing). */
  replace(route: Route) {
    this.current = route;
  }

  back() {
    const prev = this.#back.pop();
    if (!prev) return;
    this.#forward.push(this.#here());
    this.restoreScroll = prev.scroll;
    this.direction = "back";
    this.current = prev.route;
  }

  forward() {
    const next = this.#forward.pop();
    if (!next) return;
    this.#back.push(this.#here());
    this.restoreScroll = next.scroll;
    this.direction = "forward";
    this.current = next.route;
  }
}

export const router = new Router();
