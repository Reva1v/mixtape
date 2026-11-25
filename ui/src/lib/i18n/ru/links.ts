// Ссылки на YouTube Music: карточка в поиске (lib/ui/LinkCard.svelte), «Поделиться» в меню
// (lib/ui/share.ts) и блок про расширение браузера в настройках (IntegrationsSettings.svelte).
import type { links as en } from "../en/links";

export const links: typeof en = {
  link: {
    track: "Трек по ссылке",
    album: "Альбом по ссылке",
    playlist: "Плейлист по ссылке",
    artist: "Исполнитель по ссылке",
    loadFailed: "Не удалось загрузить данные трека",
    openFailed: "Не удалось открыть ссылку",
  },
  share: {
    track: "Поделиться треком",
    album: "Поделиться альбомом",
    playlist: "Поделиться плейлистом",
    artist: "Поделиться исполнителем",
    copied: "Ссылка скопирована",
    copyFailed: "Не удалось скопировать ссылку",
  },
  extension: {
    title: "Расширение для браузера",
    name: "Открывать ссылки YouTube Music в Mixtape",
    hint: "Ссылки на YouTube Music из чатов и с других сайтов открываются в Mixtape, а не в браузере.",
    openFolder: "Открыть папку",
    openFailed: "Не удалось открыть папку",
    steps: [
      "Откройте chrome://extensions (Chrome) или edge://extensions (Edge).",
      "Включите режим разработчика.",
      "Нажмите «Загрузить распакованное расширение» и выберите папку с расширением.",
    ],
  },
};
