// User preferences, mirrored from the core and applied to the page.

import { invoke } from "$lib/ipc";
import type { Prefs } from "$lib/ipc/bindings/Prefs";
import { locale } from "$lib/i18n";

class PrefsStore {
  value = $state<Prefs>({ smoothBackground: true, crossfade: false, crossfadeSec: 6, mixFullscreen: false, autoUpdate: true, iconStyle: "outline", closeToTray: true, discordPresence: true });

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
    if (this.value.language) locale.set(this.value.language);
  }
}

export const prefs = new PrefsStore();
