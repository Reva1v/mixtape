import type { updates as en } from "../en/updates";

export const updates: typeof en = {
  title: "Обновления",
  version: (v: string) => `Mixtape ${v}`,
  auto: "Проверять обновления автоматически",
  autoHint: "При запуске и раз в день. Когда устанавливать, решаете вы.",
  check: "Проверить сейчас",
  checking: "Проверяю…",
  latest: "У вас последняя версия",
  available: (v: string) => `Доступна версия Mixtape ${v}`,
  install: "Обновить",
  installing: (pct: number | null) => (pct === null ? "Скачиваю обновление…" : `Скачиваю обновление… ${pct}%`),
  restartHint: "После обновления приложение перезапустится и продолжит с того же места.",
};
