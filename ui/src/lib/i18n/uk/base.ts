import type { base as en } from "../en/base";
import { slavic } from "../plural";

export const base: typeof en = {
  appName: "Mixtape",
  nav: {
    home: "Головна",
    search: "Пошук",
    library: "Бібліотека",
    downloads: "Завантаження",
    settings: "Налаштування",
    back: "Назад",
    forward: "Вперед",
  },
  search: {
    placeholder: "Треки, альбоми, виконавці",
    hint: "Ctrl+K",
  },
  library: {
    tracks: "Треки",
    albums: "Альбоми",
    artists: "Виконавці",
    playlists: "Плейлисти",
    liked: "Вподобане",
    empty: "Бібліотека поки порожня",
    emptyHint: "Підключіть YouTube Music або вставте посилання на трек, альбом чи плейлист",
  },
  player: {
    play: "Грати",
    pause: "Пауза",
    next: "Наступний",
    prev: "Попередній",
    shuffle: "Перемішати",
    repeat: "Повтор",
    repeatOne: "Повтор треку",
    volume: "Гучність",
    mute: "Без звуку",
    queue: "Черга",
    lyrics: "Текст",
    fullscreen: "Повноекранний плеєр",
    position: "Позиція відтворення",
    nothing: "Нічого не грає",
  },
  queue: {
    title: "Черга",
    nowPlaying: "Зараз грає",
    upNext: "Далі",
    clear: "Очистити",
    building: "Збираю мікс…",
    empty: "Черга порожня",
  },
  track: {
    mix: "Увімкнути мікс",
    playNext: "Увімкнути наступним",
    addToQueue: "Додати в чергу",
    addToPlaylist: "Додати в плейлист",
    like: "Подобається",
    unlike: "Прибрати з вподобаного",
    openAlbum: "Відкрити альбом",
    openArtist: "Перейти до виконавця",
    download: "Завантажити",
    remove: "Видалити з бібліотеки",
  },
  albumType: { album: "Альбом", ep: "EP", single: "Сингл" },
  language: {
    title: "Мова",
    hint: "Меню, кнопки та розділи YouTube Music. Назви треків і виконавців не змінюються.",
  },
  common: {
    soon: "Незабаром",
    retry: "Повторити",
    cancel: "Скасувати",
    ok: "Готово",
  },
  // Mixtape inside Droplet Island (lib/ipc/embed.ts).
  island: {
    openApp: "Відкрити Mixtape окремо",
    back: "Назад до острова",
  },
  count: {
    tracks: (n: number) => `${n} ${slavic(n, "трек", "треки", "треків")}`,
  },
};
