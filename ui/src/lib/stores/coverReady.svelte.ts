// Covers that land after the answer that shows them. A catalog search comes back before its
// thumbnails are fetched; the core announces each one (`covers:ready`, the keys that landed)
// and a picture that failed to load is asked for again under a new `?v=` of its URL.
//
// Two orders are possible and both end well: the announcement after the picture failed
// (the failure waits in `missing`) and before it (the key waits in `recent`, so the failure
// that follows asks again at once). A retry is spent once per announcement, so a cover that is
// really absent does not loop.

import { SvelteMap } from "svelte/reactivity";
import { listen } from "@tauri-apps/api/event";

export function createCoverReady(max = 256) {
  const versions = new SvelteMap<string, number>();
  /** Keys whose picture failed to load and that may still be announced. */
  const missing = new Set<string>();
  /** Keys announced lately. */
  const recent = new Set<string>();

  const bump = (key: string) => versions.set(key, (versions.get(key) ?? 0) + 1);

  function remember(set: Set<string>, key: string) {
    set.delete(key);
    set.add(key);
    if (set.size > max) set.delete(set.values().next().value!);
  }

  return {
    /** Part of the cover's URL: it changes when the picture may have landed. 0 until then. */
    version: (key: string | null | undefined): number => (key ? (versions.get(key) ?? 0) : 0),

    /** `covers:ready`: these keys are in the store now. */
    ready(keys: string[]) {
      for (const key of keys) {
        if (missing.delete(key)) bump(key);
        else remember(recent, key);
      }
    },

    /** The picture of `key` failed to load at every size. */
    failed(key: string | null | undefined) {
      if (!key) return;
      if (recent.delete(key)) bump(key);
      else remember(missing, key);
    },
  };
}

export const coverReady = createCoverReady();

if (typeof window !== "undefined") {
  // No core around (the dev mock, a test): there is nothing to wait for.
  listen<string[]>("covers:ready", (e) => coverReady.ready(e.payload)).catch(() => {});
}
