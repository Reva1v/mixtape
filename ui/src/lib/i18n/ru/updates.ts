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
  island: {
    available: (v: string) => `Droplet Island ${v} доступен — с ним придёт новый Mixtape`,
    install: "Обновить остров",
    restartHint: "После обновления Droplet Island перезапустится.",
  },
  ytdlp: {
    title: "yt-dlp",
    inUse: (version: string | null, updatedOn: string | null) =>
      `${version ?? "версия неизвестна"} · ${updatedOn ? `обновлена ${updatedOn}` : "встроенная"}`,
    upToDate: "Актуальная",
    pending: (latest: string, date: string) => `Новая версия ${latest} установится сама после ${date}`,
    pendingManual: (latest: string) => `Вышла новая версия ${latest}; автообновление выключено`,
    revertedNote: (v: string) => `Вы вернули встроенную; ${v} снова не установится, более новая — установится`,
    installed: (version: string) => `Установлена ${version}`,
    revert: "Вернуть встроенную",
    reverting: "Переключаю…",
    auto: "Обновлять yt-dlp автоматически",
    autoHint: "Через 5 дней после выхода, после проверки подписи. Можно вернуть встроенную.",
    toastPending: (latest: string, installed: string, date: string) =>
      `Вышла новая yt-dlp ${latest} (у вас ${installed}). Установится сама после ${date}`,
    toastManual: (latest: string, installed: string) =>
      `Вышла новая yt-dlp ${latest} (у вас ${installed}). Сама она не ставится — попросите Claude проверить и обновить`,
    toastInstalled: (version: string) => `Установлена yt-dlp ${version}`,
    toastRolledBack: (bad: string, restored: string) => `Новая yt-dlp ${bad} не работала, вернул ${restored}`,
  },
};
