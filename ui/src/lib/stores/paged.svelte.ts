// Lazily paged rows for virtual lists: a row asks for its index, the page that holds
// it is fetched once, and everything stays in `$state.raw` pages (no deep proxies).

import { SvelteMap } from "svelte/reactivity";

export const PAGE = 100;

export class Paged<T> {
  count = $state(0);
  #pages = new SvelteMap<number, readonly T[]>();
  #loading = new Set<number>();
  #fetch: (offset: number, limit: number) => Promise<T[]>;
  #generation = 0;

  constructor(count: number, fetch: (offset: number, limit: number) => Promise<T[]>) {
    this.count = count;
    this.#fetch = fetch;
  }

  get(index: number): T | undefined {
    const p = Math.floor(index / PAGE);
    const page = this.#pages.get(p);
    if (!page) this.#load(p);
    return page?.[index % PAGE];
  }

  #load(p: number) {
    if (this.#loading.has(p)) return;
    this.#loading.add(p);
    const gen = this.#generation;
    // Defer so `get` stays side-effect free during rendering.
    queueMicrotask(async () => {
      try {
        const rows = await this.#fetch(p * PAGE, PAGE);
        if (gen === this.#generation) this.#pages.set(p, rows);
      } finally {
        this.#loading.delete(p);
      }
    });
  }

  /** Drops cached pages, e.g. after `library:changed`. */
  reset(count: number, fetch?: (offset: number, limit: number) => Promise<T[]>) {
    this.#generation++;
    this.#pages.clear();
    this.#loading.clear();
    this.count = count;
    if (fetch) this.#fetch = fetch;
  }
}

/** An in-memory list behind the same interface, for albums and playlists. */
export class Fixed<T> {
  items = $state.raw<readonly T[]>([]);
  constructor(items: readonly T[]) {
    this.items = items;
  }
  get count() {
    return this.items.length;
  }
  get(index: number): T | undefined {
    return this.items[index];
  }
}

export type RowSource<T> = { readonly count: number; get(index: number): T | undefined };
