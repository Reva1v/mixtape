// User preferences, mirrored from the core and applied to the page.

import { invoke } from "$lib/ipc";
import type { Prefs } from "$lib/ipc/bindings/Prefs";

class PrefsStore {
  value = $state<Prefs>({ smoothBackground: true, crossfade: false, crossfadeSec: 6 });

  async init() {
    this.value = await invoke<Prefs>("prefs_get");
    this.#apply();
  }

  async update(patch: Partial<Prefs>) {
    this.value = { ...this.value, ...patch };
    this.#apply();
    await invoke("prefs_set", { prefs: this.value });
  }

  #apply() {
    document.documentElement.classList.toggle("instant-bg", !this.value.smoothBackground);
  }
}

export const prefs = new PrefsStore();
