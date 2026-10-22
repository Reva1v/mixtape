// Statistics screen (screens/Stats.svelte). English is the reference shape.
import { english } from "../plural";

export const stats = {
  stats: {
    title: "Statistics",
    period: {
      label: "Period",
      week: "Last 7 days",
      month: "Last 30 days",
      year: "This year",
      all: "All time",
    },
    /** The device filter and the per-device split under the totals. */
    allDevices: "All devices",
    thisDevice: "This PC",
    otherDevice: "Other device",
    total: "Total",
    listened: "Listening time",
    plays: "Plays",
    tracks: "Tracks",
    artists: "Artists",
    topDay: "Biggest day",
    topTracks: "Top tracks",
    topArtists: "Top artists",
    topAlbums: "Top albums",
    byHour: "By time of day",
    byWeekday: "By day of the week",
    hours: "h",
    minutes: "min",
    /** `shown` is `n` already formatted for the interface language. */
    playCount: (n: number, shown: string) => `${shown} ${english(n, "play")}`,
    empty: "Nothing here yet",
    emptyHint: "Statistics fill up as you listen: a track counts after 30 seconds.",
    emptyPeriod: "Nothing played in this period",
    showAll: "Show all time",
  },
};
