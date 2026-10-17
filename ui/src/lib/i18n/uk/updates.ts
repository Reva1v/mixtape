import type { updates as en } from "../en/updates";

export const updates: typeof en = {
  title: "Оновлення",
  version: (v: string) => `Mixtape ${v}`,
  auto: "Перевіряти оновлення автоматично",
  autoHint: "Під час запуску та раз на день. Коли встановлювати, вирішуєте ви.",
  check: "Перевірити зараз",
  checking: "Перевіряю…",
  latest: "У вас остання версія",
  available: (v: string) => `Доступна версія Mixtape ${v}`,
  install: "Оновити",
  installing: (pct: number | null) => (pct === null ? "Завантажую оновлення…" : `Завантажую оновлення… ${pct}%`),
  restartHint: "Після оновлення застосунок перезапуститься й продовжить з того ж місця.",
};
