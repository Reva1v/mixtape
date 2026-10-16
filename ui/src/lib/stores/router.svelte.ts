// A tiny in-memory router: a desktop app has no URL bar, just screens and history.

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

class Router {
  #back = $state<Route[]>([]);
  #forward = $state<Route[]>([]);
  current = $state<Route>({ name: "home" });

  get canBack() {
    return this.#back.length > 0;
  }
  get canForward() {
    return this.#forward.length > 0;
  }

  go(route: Route) {
    if (JSON.stringify(route) === JSON.stringify(this.current)) return;
    this.#back.push(this.current);
    if (this.#back.length > 50) this.#back.shift();
    this.#forward = [];
    this.current = route;
  }

  /** Same as `go`, but replaces the current entry (tabs, search typing). */
  replace(route: Route) {
    this.current = route;
  }

  back() {
    const prev = this.#back.pop();
    if (!prev) return;
    this.#forward.push(this.current);
    this.current = prev;
  }

  forward() {
    const next = this.#forward.pop();
    if (!next) return;
    this.#back.push(this.current);
    this.current = next;
  }
}

export const router = new Router();
