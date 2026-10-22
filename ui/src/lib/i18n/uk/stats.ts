import type { stats as en } from "../en/stats";
import { slavic } from "../plural";

export const stats: typeof en = {
  stats: {
    title: "Статистика",
    period: {
      label: "Період",
      week: "За 7 днів",
      month: "За 30 днів",
      year: "Цього року",
      all: "За весь час",
    },
    allDevices: "Усі пристрої",
    thisDevice: "Цей ПК",
    otherDevice: "Інший пристрій",
    total: "Усього",
    listened: "Час прослуховування",
    plays: "Прослуховування",
    tracks: "Треки",
    artists: "Виконавці",
    topDay: "Найактивніший день",
    topTracks: "Топ треків",
    topArtists: "Топ виконавців",
    topAlbums: "Топ альбомів",
    byHour: "За часом доби",
    byWeekday: "За днями тижня",
    hours: "год",
    minutes: "хв",
    playCount: (n: number, shown: string) =>
      `${shown} ${slavic(n, "прослуховування", "прослуховування", "прослуховувань")}`,
    empty: "Поки порожньо",
    emptyHint: "Статистика наповнюється, поки ви слухаєте: трек зараховується після 30 секунд.",
    emptyPeriod: "За цей період нічого не прослухано",
    showAll: "Показати за весь час",
  },
};
