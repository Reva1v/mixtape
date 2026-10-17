// The tray menu's strings: only the base and tray dictionaries, so its page does not
// carry the whole app's text.
import type { Lang } from "$lib/ipc/bindings/Lang";
import { base as enBase } from "$lib/i18n/en/base";
import { tray as enTray } from "$lib/i18n/en/tray";
import { base as ukBase } from "$lib/i18n/uk/base";
import { tray as ukTray } from "$lib/i18n/uk/tray";
import { base as ruBase } from "$lib/i18n/ru/base";
import { tray as ruTray } from "$lib/i18n/ru/tray";

const dicts = {
  en: { ...enBase, ...enTray },
  uk: { ...ukBase, ...ukTray },
  ru: { ...ruBase, ...ruTray },
} satisfies Record<Lang, unknown>;

export type Strings = (typeof dicts)["en"];

export function strings(lang: Lang): Strings {
  return dicts[lang] ?? dicts.en;
}
