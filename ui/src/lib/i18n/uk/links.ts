// Посилання на YouTube Music: картка в пошуку (lib/ui/LinkCard.svelte), «Поділитися» в меню
// (lib/ui/share.ts) і блок про розширення браузера в налаштуваннях (IntegrationsSettings.svelte).
import type { links as en } from "../en/links";

export const links: typeof en = {
  link: {
    track: "Трек за посиланням",
    album: "Альбом за посиланням",
    playlist: "Плейлист за посиланням",
    artist: "Виконавець за посиланням",
    loadFailed: "Не вдалося завантажити дані треку",
    openFailed: "Не вдалося відкрити посилання",
  },
  share: {
    track: "Поділитися треком",
    album: "Поділитися альбомом",
    playlist: "Поділитися плейлистом",
    artist: "Поділитися виконавцем",
    copied: "Посилання скопійовано",
    copyFailed: "Не вдалося скопіювати посилання",
  },
  extension: {
    title: "Розширення для браузера",
    name: "Відкривати посилання YouTube Music у Mixtape",
    hint: "Посилання на YouTube Music із чатів та інших сайтів відкриваються в Mixtape, а не в браузері.",
    openFolder: "Відкрити теку",
    openFailed: "Не вдалося відкрити теку",
    steps: [
      "Відкрийте chrome://extensions (Chrome) або edge://extensions (Edge).",
      "Увімкніть режим розробника.",
      "Натисніть «Завантажити розпаковане розширення» та виберіть теку з розширенням.",
    ],
  },
};
