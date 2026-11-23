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
  island: {
    available: (v: string) => `Droplet Island ${v} доступний — з ним прийде новий Mixtape`,
    install: "Оновити острів",
    restartHint: "Після оновлення Droplet Island перезапуститься.",
  },
  ytdlp: {
    title: "yt-dlp",
    inUse: (version: string | null, updatedOn: string | null) =>
      `${version ?? "версія невідома"} · ${updatedOn ? `оновлена ${updatedOn}` : "вбудована"}`,
    upToDate: "Актуальна",
    pending: (latest: string, date: string) => `Нова версія ${latest} встановиться сама після ${date}`,
    pendingManual: (latest: string) => `Вийшла нова версія ${latest}; автооновлення вимкнено`,
    revertedNote: (v: string) => `Ви повернули вбудовану; ${v} знову не встановиться, новіша — встановиться`,
    installed: (version: string) => `Встановлено ${version}`,
    revert: "Повернути вбудовану",
    reverting: "Перемикаю…",
    auto: "Оновлювати yt-dlp автоматично",
    autoHint: "Через 5 днів після виходу, після перевірки підпису. Можна повернути вбудовану.",
    toastPending: (latest: string, installed: string, date: string) =>
      `Вийшла нова yt-dlp ${latest} (у вас ${installed}). Встановиться сама після ${date}`,
    toastManual: (latest: string, installed: string) =>
      `Вийшла нова yt-dlp ${latest} (у вас ${installed}). Сама вона не встановлюється — попросіть Claude перевірити й оновити`,
    toastInstalled: (version: string) => `Встановлено yt-dlp ${version}`,
    toastRolledBack: (bad: string, restored: string) => `Нова yt-dlp ${bad} не працювала, повернув ${restored}`,
  },
};
