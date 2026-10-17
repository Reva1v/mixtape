import type { stats as en } from "../en/stats";
import { slavic } from "../plural";

export const stats: typeof en = {
  stats: {
    title: "Статистика",
    period: {
      label: "Период",
      week: "За 7 дней",
      month: "За 30 дней",
      year: "В этом году",
      all: "За всё время",
    },
    listened: "Время прослушивания",
    plays: "Прослушивания",
    tracks: "Треки",
    artists: "Исполнители",
    topDay: "Самый активный день",
    topTracks: "Топ треков",
    topArtists: "Топ исполнителей",
    topAlbums: "Топ альбомов",
    byHour: "По времени суток",
    byWeekday: "По дням недели",
    hours: "ч",
    minutes: "мин",
    playCount: (n: number, shown: string) =>
      `${shown} ${slavic(n, "прослушивание", "прослушивания", "прослушиваний")}`,
    empty: "Пока пусто",
    emptyHint: "Статистика наполняется, пока вы слушаете: трек засчитывается после 30 секунд.",
    emptyPeriod: "За этот период ничего не прослушано",
    showAll: "Показать за всё время",
  },
};
