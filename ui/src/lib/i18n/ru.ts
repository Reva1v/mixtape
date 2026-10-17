// All interface strings live here. The app is Russian-only (FR-028).
export const t = {
  appName: "Mixtape",
  nav: {
    home: "Главная",
    search: "Поиск",
    library: "Библиотека",
    downloads: "Загрузки",
    settings: "Настройки",
    back: "Назад",
    forward: "Вперёд",
  },
  search: {
    placeholder: "Треки, альбомы, исполнители",
    hint: "Ctrl+K",
  },
  library: {
    tracks: "Треки",
    albums: "Альбомы",
    artists: "Исполнители",
    playlists: "Плейлисты",
    liked: "Понравившиеся",
    empty: "Библиотека пока пуста",
    emptyHint: "Подключите YouTube Music или вставьте ссылку на трек, альбом или плейлист",
  },
  player: {
    play: "Играть",
    pause: "Пауза",
    next: "Следующий",
    prev: "Предыдущий",
    shuffle: "Перемешать",
    repeat: "Повтор",
    repeatOne: "Повтор трека",
    volume: "Громкость",
    mute: "Без звука",
    queue: "Очередь",
    lyrics: "Текст",
    fullscreen: "Полноэкранный плеер",
    nothing: "Ничего не играет",
  },
  queue: {
    title: "Очередь",
    nowPlaying: "Сейчас играет",
    upNext: "Далее",
    clear: "Очистить",
    building: "Собираю микс…",
    empty: "Очередь пуста",
  },
  track: {
    mix: "Включить микс",
    playNext: "Включить следующим",
    addToQueue: "Добавить в очередь",
    addToPlaylist: "Добавить в плейлист",
    like: "Нравится",
    unlike: "Убрать из понравившихся",
    openAlbum: "Открыть альбом",
    openArtist: "Перейти к исполнителю",
    download: "Скачать",
    remove: "Удалить из библиотеки",
  },
  common: {
    soon: "Скоро",
    retry: "Повторить",
    cancel: "Отмена",
    ok: "Готово",
  },
} as const;

/** Russian plural form: plural(1, "трек", "трека", "треков") → "трек". */
export function plural(n: number, one: string, few: string, many: string): string {
  const m10 = n % 10;
  const m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}

export const tracks = (n: number) => `${n} ${plural(n, "трек", "трека", "треков")}`;
