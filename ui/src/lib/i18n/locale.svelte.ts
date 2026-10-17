// Interface language. `t` always reads the current dictionary, so whatever renders from
// it follows a change in settings without a reload.
import type { Lang } from "$lib/ipc/bindings/Lang";
import { en } from "./en";
import { uk } from "./uk";
import { ru } from "./ru";

export type Dict = typeof en;
const dicts: Record<Lang, Dict> = { en, uk, ru };

/** In the order the settings list shows them. */
export const LANGS: { id: Lang; name: string }[] = [
  { id: "en", name: "English" },
  { id: "uk", name: "Українська" },
  { id: "ru", name: "Русский" },
];

const TAGS: Record<Lang, string> = { en: "en-US", uk: "uk-UA", ru: "ru-RU" };

/** Until the core says otherwise: the webview's language, which is Windows' one. */
function guess(): Lang {
  const l = navigator.language.slice(0, 2).toLowerCase();
  return l === "uk" ? "uk" : l === "ru" || l === "be" ? "ru" : "en";
}

class Locale {
  lang = $state<Lang>(guess());

  /** BCP 47 tag for `Intl` and `toLocaleString`. */
  get tag() {
    return TAGS[this.lang];
  }

  set(lang: Lang) {
    this.lang = lang;
    document.documentElement.lang = lang;
  }
}

export const locale = new Locale();

export const t: Dict = new Proxy({} as Dict, {
  get: (_, key) => dicts[locale.lang][key as keyof Dict],
});
