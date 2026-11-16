// What the user looked for and opened from search, shown when the search box is empty.
// The list lives in the core's database, the same as on the phone: this store keeps a copy,
// changes it at once and lets the core confirm (the core's answer is the whole new list), and
// reloads it when a phone sync changed it (`recent:changed`).

import { invoke, on } from "$lib/ipc";
import { added, readLegacy, removed, type RecentItem } from "./recentList";

export type { RecentItem } from "./recentList";

// The list this interface kept in the browser before the database (v2: subtitles no longer
// carry the language-bound kind label). Moved to the database once, then deleted.
const LEGACY_KEY = "mixtape.recentSearch.v2";

class RecentSearch {
  items = $state<RecentItem[]>([]);
  /** Counts changes, so an answer to an older request doesn't overwrite a newer list. */
  #version = 0;

  constructor() {
    void this.#start();
  }

  add(item: RecentItem) {
    const next = added(this.items, item);
    if (next === this.items) return;
    this.items = next;
    void this.#send("recent_add", { item });
  }

  remove(item: RecentItem) {
    this.items = removed(this.items, item);
    void this.#send("recent_remove", { item });
  }

  clear() {
    this.items = [];
    void this.#send("recent_clear");
  }

  async #start() {
    await this.#load();
    await this.#migrate();
    try {
      await on("recent:changed", () => void this.#load());
    } catch {
      // No event bus (a plain page): the list is still right for what this window changes.
    }
  }

  /** The list from the core, unless the user changed it meanwhile. */
  async #load() {
    const version = this.#version;
    try {
      const list = await invoke<RecentItem[]>("recent_searches");
      if (version === this.#version) this.items = list;
    } catch {
      // The core is not there yet or not in this host: keep what is shown.
    }
  }

  /** A change goes to the core; its answer replaces the list, a failure reloads it. */
  async #send(cmd: string, args?: Record<string, unknown>) {
    const version = ++this.#version;
    try {
      const list = await invoke<RecentItem[]>(cmd, args);
      if (version === this.#version) this.items = list;
    } catch {
      if (version === this.#version) await this.#load();
    }
  }

  /** The old browser list goes to the database once; the key is deleted only after that worked. */
  async #migrate() {
    let raw: string | null;
    try {
      raw = localStorage.getItem(LEGACY_KEY);
    } catch {
      return;
    }
    if (raw === null) return;
    const legacy = readLegacy(raw);
    try {
      if (legacy.length) {
        const list = await invoke<RecentItem[]>("recent_import", { items: legacy });
        this.#version++;
        this.items = list;
      }
      localStorage.removeItem(LEGACY_KEY);
    } catch {
      // Tried again at the next start.
    }
  }
}

export const recentSearch = new RecentSearch();
