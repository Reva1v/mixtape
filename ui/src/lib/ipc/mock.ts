// Dev-only fake core for looking at screens in a plain browser: open the Vite page with
// `?mock` (never bundled into the app, see main.ts). It answers with a made-up catalogue
// (invented artists, covers drawn on a canvas), so screenshots never show anyone's
// library. Commands it doesn't know fail the way a missing core does.
//
// URL switches: `lang=en|uk|ru` (interface language), `mock=off` (no account),
// `scene=home|album|library|stats|fullscreen|queue|mini` (open that screen with a track playing).

import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { Channel } from "@tauri-apps/api/core";
import type { AccountStatus, DownloadsProgress, FailedJob, SyncPreview } from "./sync";
import type { Album } from "./bindings/Album";
import type { HomePage } from "./bindings/HomePage";
import type { HomeShelf } from "./bindings/HomeShelf";
import type { Lang } from "./bindings/Lang";
import type { ListeningStats } from "./bindings/ListeningStats";
import type { Lyrics } from "./bindings/Lyrics";
import type { PlaybackState } from "./bindings/PlaybackState";
import type { QueueView } from "./bindings/QueueView";
import type { Track } from "./bindings/Track";
import type { ViewKind } from "./bindings/ViewKind";
import { setCoverSource } from "./index";
import { router } from "$lib/stores/router.svelte";
import { layout } from "$lib/stores/layout.svelte";

const params = new URLSearchParams(location.search);
const lang = (["en", "uk", "ru"].includes(params.get("lang") ?? "") ? params.get("lang") : "en") as Lang;
const scene = params.get("scene");

// ---- Covers: abstract art drawn from each album's palette ----

const PALETTES: string[][] = [
  ["#ff6b6b", "#5f27cd", "#feca57", "#1e1e3f", "#ff9ff3"],
  ["#0abde3", "#10ac84", "#c8d6e5", "#0b3d4f", "#48dbfb"],
  ["#f368e0", "#341f97", "#ff9f43", "#130f40", "#ee5253"],
  ["#feca57", "#ff6b6b", "#fff3e0", "#6d214f", "#ff9f43"],
  ["#54a0ff", "#2e86de", "#e0f2ff", "#0c2461", "#82ccdd"],
  ["#1dd1a1", "#576574", "#f5f6fa", "#222f3e", "#c8f7c5"],
  ["#ff7979", "#badc58", "#f6e58d", "#30336b", "#e056fd"],
  ["#e17055", "#2d3436", "#fab1a0", "#130f10", "#fdcb6e"],
  ["#a29bfe", "#6c5ce7", "#ffeaa7", "#1b1464", "#fd79a8"],
  ["#00cec9", "#e84393", "#ffeaa7", "#2d1b3d", "#81ecec"],
];

function hash(s: string): number {
  let h = 2166136261;
  for (const c of s) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return h >>> 0;
}

function rng(seed: number) {
  return () => ((seed = Math.imul(seed ^ (seed >>> 15), 2246822507) + 0x9e3779b9) >>> 0) / 2 ** 32;
}

const drawn = new Map<string, string>();

function drawCover(key: string, size: number): string {
  const id = `${key}/${size}`;
  const hit = drawn.get(id);
  if (hit) return hit;
  const h = hash(key);
  const p = PALETTES[h % PALETTES.length];
  const rnd = rng(h);
  const c = document.createElement("canvas");
  c.width = c.height = size;
  const g = c.getContext("2d")!;
  const s = size;
  const bg = g.createLinearGradient(0, 0, s, s);
  bg.addColorStop(0, p[3]);
  bg.addColorStop(1, p[1]);
  g.fillStyle = bg;
  g.fillRect(0, 0, s, s);
  // Soft colour blobs.
  g.filter = `blur(${s * 0.08}px)`;
  for (let i = 0; i < 4; i++) {
    const x = rnd() * s;
    const y = rnd() * s;
    const r = s * (0.25 + rnd() * 0.35);
    const blob = g.createRadialGradient(x, y, 0, x, y, r);
    blob.addColorStop(0, p[[0, 2, 4, 0][i]] + "dd");
    blob.addColorStop(1, p[[0, 2, 4, 0][i]] + "00");
    g.fillStyle = blob;
    g.fillRect(0, 0, s, s);
  }
  g.filter = "none";
  // One shape per cover: a striped sun, rings or a tilted band.
  g.save();
  switch (h % 3) {
    case 0: {
      const cx = s * (0.35 + rnd() * 0.3);
      const cy = s * (0.4 + rnd() * 0.2);
      const r = s * 0.24;
      const sun = g.createLinearGradient(0, cy - r, 0, cy + r);
      sun.addColorStop(0, p[2]);
      sun.addColorStop(1, p[0]);
      g.fillStyle = sun;
      g.beginPath();
      g.arc(cx, cy, r, 0, Math.PI * 2);
      g.fill();
      g.globalCompositeOperation = "destination-out";
      for (let i = 0; i < 5; i++) g.fillRect(0, cy + r * (0.1 + i * 0.2), s, r * (0.03 + i * 0.025));
      break;
    }
    case 1: {
      g.strokeStyle = p[2] + "cc";
      const cx = s * (0.3 + rnd() * 0.4);
      const cy = s * (0.3 + rnd() * 0.4);
      for (let i = 1; i <= 6; i++) {
        g.lineWidth = s * 0.008 * (7 - i) * 0.6;
        g.globalAlpha = 1 - i * 0.13;
        g.beginPath();
        g.arc(cx, cy, s * 0.06 * i, 0, Math.PI * 2);
        g.stroke();
      }
      break;
    }
    default: {
      g.translate(s / 2, s / 2);
      g.rotate(-0.5 + rnd());
      const band = g.createLinearGradient(-s, 0, s, 0);
      band.addColorStop(0, p[0] + "00");
      band.addColorStop(0.5, p[2] + "ee");
      band.addColorStop(1, p[4] + "00");
      g.fillStyle = band;
      g.fillRect(-s, -s * 0.07, s * 2, s * 0.14);
    }
  }
  g.restore();
  // Film grain.
  const img = g.getImageData(0, 0, s, s);
  for (let i = 0; i < img.data.length; i += 4) {
    const n = (rnd() - 0.5) * 18;
    img.data[i] += n;
    img.data[i + 1] += n;
    img.data[i + 2] += n;
  }
  g.putImageData(img, 0, 0);
  const url = c.toDataURL("image/jpeg", 0.9);
  drawn.set(id, url);
  return url;
}

setCoverSource(drawCover);

const paletteOf = (key: string) => PALETTES[hash(key) % PALETTES.length];

// ---- Catalogue ----

const ARTISTS = [
  "Neon Harbor",
  "Juno Vale",
  "Static Bloom",
  "Coastline Radio",
  "Mira Sol",
  "Oslo Nights",
  "Pale Arcade",
  "Marlow Reeves",
  "Halcyon Drift",
  "Kite Theory",
];
const artist = (i: number) => ({ id: `UCdemo${i}`, name: ARTISTS[i] });

const ALBUM_TITLES: [string, number, number, "album" | "ep" | "single"][] = [
  ["Afterglow Avenue", 0, 2024, "album"],
  ["Low Tide Letters", 1, 2023, "album"],
  ["Paper Satellites", 2, 2025, "album"],
  ["Midnight Transit", 3, 2022, "ep"],
  ["Golden Hour Tapes", 4, 2024, "album"],
  ["Northern Static", 5, 2021, "album"],
  ["Soft Machines", 6, 2025, "ep"],
  ["Blue Room Sessions", 7, 2020, "album"],
  ["Wildflower Season", 8, 2023, "album"],
  ["Night Swim", 9, 2025, "single"],
];

const albums: Album[] = ALBUM_TITLES.map(([title, a, year, albumType], i) => ({
  id: `MPREdemo${i}`,
  title,
  artists: [artist(a)],
  year,
  albumType,
  coverKey: `demo-cover-${i}`,
  palette: paletteOf(`demo-cover-${i}`),
  trackCount: 10,
}));

const SONGS = [
  "City Lights Fade",
  "Glass Ocean",
  "Slow Motion Summer",
  "Paper Planes at Dawn",
  "Neon Rain",
  "Weightless",
  "Postcards",
  "Run to the Coast",
  "Velvet Hours",
  "Echoes in Blue",
  "Satellite Heart",
  "Golden Static",
  "Afterimage",
  "Lanterns",
  "Heatwave Radio",
  "Northbound",
  "Small Hours",
  "Driftwood",
  "Falling Upward",
  "Polaroid",
];

/** Track `n` of album `al`. */
function track(al: number, n: number): Track {
  const album = albums[al];
  return {
    id: `demo${al}t${n}`.padEnd(11, "x"),
    title: SONGS[(al * 7 + n) % SONGS.length],
    artists: album.artists,
    album: { id: album.id, title: album.title },
    trackNo: n + 1,
    durationMs: 150_000 + ((al * 37 + n * 53) % 140) * 1000,
    coverKey: album.coverKey,
    state: (al + n) % 4 ? "downloaded" : "remote",
    liked: (al + n) % 3 === 0,
  };
}
const albumTracks = (al: number) => Array.from({ length: albums[al].albumType === "single" ? 1 : 10 }, (_, n) => track(al, n));
const allTracks = albums.flatMap((_, i) => albumTracks(i));

const NOW = track(0, 0);
NOW.liked = true;
NOW.state = "downloaded";

// ---- Home ----

const SHELVES: Record<Lang, string[]> = {
  en: ["Quick picks", "Mixed for you", "Listen again", "Albums for you", "Artists and playlists"],
  uk: ["Швидкий вибір", "Мікси для вас", "Слухати знову", "Альбоми для вас", "Виконавці та плейлисти"],
  ru: ["Быстрый выбор", "Миксы для вас", "Послушать ещё раз", "Альбомы для вас", "Исполнители и плейлисты"],
};
const MIXES: Record<Lang, string> = { en: "My Mix", uk: "Мій мікс", ru: "Мой микс" };
const AND_MORE: Record<Lang, string> = { en: "and more", uk: "та інші", ru: "и не только" };

const homePage: HomePage = {
  likedCount: 412,
  recent: albums.slice(0, 6),
  added: albums.slice(4, 10),
  top: [0, 1, 2, 3, 4].map((i) => track(i, i + 1)),
};

const range = (from: number, n: number) => Array.from({ length: n }, (_, i) => from + i);
const [quick, mixed, again, forYou, more] = SHELVES[lang];
const homeShelves: HomeShelf[] = [
  { title: quick, items: range(0, 16).map((n) => ({ kind: "track", track: track(n % 10, (n * 3) % 10) })) },
  {
    title: mixed,
    items: range(1, 6).map((n) => ({
      kind: "mix",
      playlistId: `RDTMAKdemo${n}`,
      title: `${MIXES[lang]} ${n}`,
      subtitle: `${ARTISTS[n]}, ${ARTISTS[n + 2]} ${AND_MORE[lang]}`,
      coverKey: `demo-mix-${n}`,
    })),
  },
  { title: again, items: range(0, 8).map((n) => ({ kind: "track", track: track((n + 3) % 10, n) })) },
  {
    title: forYou,
    items: albums.slice(2, 10).map((a) => ({ kind: "album", id: a.id, title: a.title, artists: a.artists, year: a.year, coverKey: a.coverKey })),
  },
  {
    title: more,
    items: [
      ...range(0, 3).map((n) => ({ kind: "artist" as const, id: artist(n).id, name: artist(n).name, coverKey: `demo-artist-${n}` })),
      ...range(1, 4).map((n) => ({ kind: "playlist" as const, id: `VLPLdemo${n}`, title: SONGS[n + 4], subtitle: "YouTube Music", coverKey: `demo-pl-${n}` })),
    ],
  },
];

// ---- Player ----

const state: PlaybackState = {
  trackId: NOW.id,
  status: "playing",
  positionMs: 74_000,
  durationMs: NOW.durationMs,
  volume: 0.8,
  repeat: "off",
  shuffle: false,
  timestampMs: Date.now(),
};

const queue: QueueView = {
  items: [NOW, ...albumTracks(0).slice(1), ...albumTracks(4).slice(0, 4)].map((track, uid) => ({
    uid,
    origin: uid < 10 ? "context" : "mix",
    track,
  })),
  current: 0,
  context: { kind: "mix", id: NOW.id },
  chips: (lang === "en" ? ["All", "Familiar", "Discover", "Popular"] : lang === "uk" ? ["Усе", "Знайоме", "Нове", "Популярне"] : ["Все", "Знакомое", "Новое", "Популярное"]).map((title, i) => ({
    id: `chip${i}`,
    title,
    selected: i === 0,
  })),
};

const LYRICS = [
  "Streetlights hum a slower song",
  "The avenue is warm and wide",
  "We trace the places we belong",
  "With half the city on our side",
  "",
  "And when the city lights fade",
  "I'll keep the colour of your name",
  "Every corner that we made",
  "Still glowing in the afterglow",
  "",
  "Paper tickets, open sky",
  "A radio that only plays",
  "The summer that we can't untie",
  "The neon of our better days",
  "",
  "And when the city lights fade",
  "I'll keep the colour of your name",
];
const lyrics: Lyrics = {
  synced: true,
  instrumental: false,
  source: "lrclib",
  lines: LYRICS.map((text, i) => ({ ms: 12_000 + i * 6_500, text })),
};

// ---- Sync and downloads (Downloads screen) ----

let account: AccountStatus = { connected: params.get("mock") !== "off" };
const named = (): AccountStatus => ({ connected: true, name: "Demo User", lastSyncAt: Date.now() - 3_600_000 });
if (account.connected) account = named();

const preview: SyncPreview = {
  liked: 412,
  albums: 23,
  artists: 9,
  playlists: [
    { id: "PL1", title: "Morning Run", origin: "ytm", trackCount: 48 },
    { id: "PL2", title: "Late Night Drive", origin: "ytm", trackCount: 112 },
    { id: "PL3", title: "A playlist with a very long name to check how the row truncates it", origin: "ytm", trackCount: 7 },
  ],
};

let failed: FailedJob[] = [
  { id: 1, trackId: "aaaaaaaaaaa", title: SONGS[3], artists: ARTISTS[1], error: "Not available in your country", attempts: 3 },
  { id: 2, trackId: "bbbbbbbbbbb", title: SONGS[8], artists: `${ARTISTS[2]}, ${ARTISTS[5]}`, error: "Blocked by the copyright owner", attempts: 3 },
];

const progress: DownloadsProgress = { running: true, paused: false, done: 57, total: 140, failed: failed.length, bytesPerSec: 0 };
let channel: Channel<DownloadsProgress> | null = null;
let bytes = 0;

/** Fake source sync: a few seconds of reading, then covers with a counter. */
let syncTicks = 0;

setInterval(() => {
  if (syncTicks > 0) {
    syncTicks--;
    progress.sync = syncTicks > 20 ? { label: "Reading likes", done: 0, total: 0 } : syncTicks > 0 ? { label: "Downloading covers", done: 20 - syncTicks, total: 20 } : undefined;
    channel?.onmessage({ ...progress, failed: failed.length });
  }
  if (!progress.running || progress.paused) return;
  const total = 5_400_000;
  bytes += 260_000 + Math.random() * 120_000;
  if (bytes >= total) {
    bytes = 0;
    progress.done = Math.min(progress.total, progress.done + 1);
  }
  progress.bytesPerSec = 1_250_000 + Math.random() * 300_000;
  progress.current = { trackId: "ddddddddddd", title: SONGS[progress.done % SONGS.length], bytesDone: bytes, bytesTotal: total };
  channel?.onmessage({ ...progress, failed: failed.length });
}, 250);

// ---- Listening statistics ----

/** A made-up recap; longer periods scale the numbers up. `?stats=empty` shows the empty state. */
function demoStats(period: string): ListeningStats {
  const empty: ListeningStats = { listenedMs: 0, plays: 0, tracks: 0, artists: 0, topTracks: [], topArtists: [], topAlbums: [], byHour: Array(24).fill(0), byWeekday: Array(7).fill(0) };
  if (params.get("stats") === "empty") return empty;
  const scale = { week: 1, month: 4.3, year: 38, all: 52 }[period] ?? 1;
  const n = (x: number) => Math.round(x * scale);
  const MIN = 60_000;
  // An evening listener: a morning commute bump, a quiet afternoon, most after dinner.
  const hourShape = [3, 1, 0, 0, 0, 0, 1, 6, 9, 5, 3, 4, 5, 3, 2, 3, 5, 7, 9, 12, 14, 13, 10, 6];
  const dayShape = [8, 7, 9, 8, 12, 15, 11];
  const topTracks = [0, 4, 2, 7, 1, 8, 3, 5, 9, 6].map((al, i) => {
    const t = track(al, (al * 3 + i) % (albums[al].albumType === "single" ? 1 : 10));
    const plays = n(14 - i) + 1;
    return { track: t, plays, listenedMs: plays * t.durationMs * 0.92 };
  });
  const plays = n(212);
  const listenedMs = n(212 * 3.4 * MIN);
  return {
    listenedMs,
    plays,
    tracks: n(118),
    artists: Math.min(ARTISTS.length * 4, n(31)),
    topTracks,
    topArtists: [0, 4, 2, 7, 1, 8, 3, 5].map((a, i) => ({ id: artist(a).id, name: ARTISTS[a], imageKey: `demo-artist-${a}`, plays: n(40 - i * 4), listenedMs: n((40 - i * 4) * 3.5 * MIN) })),
    topAlbums: [0, 4, 2, 7, 1, 8, 3].map((al, i) => ({ album: albums[al], plays: n(34 - i * 4), listenedMs: n((34 - i * 4) * 3.5 * MIN) })),
    byHour: hourShape.map((x) => (x * listenedMs) / 141),
    byWeekday: dayShape.map((x) => (x * listenedMs) / 70),
    topDay: { dayMs: new Date(2026, 9, 10).getTime(), plays: 31, listenedMs: 196 * MIN },
  };
}

// ---- Library views ----

const views = new Map<number, unknown[]>();
function view(kind: ViewKind): unknown[] {
  switch (kind) {
    case "albums":
      return albums;
    case "artists":
      return ARTISTS.map((name, i) => ({ id: artist(i).id, name, imageKey: `demo-artist-${i}`, inLibrary: true, palette: [] }));
    case "liked":
      return allTracks.filter((t) => t.liked);
    case "playlists":
      return [];
    default:
      return allTracks;
  }
}

mockIPC(
  (cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case "app_ready":
        return null;
      case "prefs_get":
        return { smoothBackground: true, crossfade: false, crossfadeSec: 6, mixFullscreen: false, autoUpdate: true, iconStyle: "outline", language: lang, closeToTray: true, discordPresence: true };
      case "prefs_set":
        return null;
      case "autostart_get":
        return "off";
      case "autostart_set":
        return a.mode;
      case "window_is_mini":
        return scene === "mini";
      case "window_set_mini":
        void emit("window:mini", a.on);
        return null;
      case "account_status":
        return account;
      case "account_connect_webview":
        return new Promise((r) => setTimeout(() => r((account = named())), 1200));
      case "account_connect_cookies":
        if (!String(a.cookiesTxt).includes("youtube")) throw { kind: "auth", message: "No youtube.com cookies in the text" };
        return (account = named());
      case "account_disconnect":
        account = { connected: false };
        return null;
      case "sync_preview":
        return new Promise((r) => setTimeout(() => r(preview), 600));
      case "sync_start":
        syncTicks = 32;
        return null;
      case "downloads_resume":
        progress.running = true;
        progress.paused = false;
        return null;
      case "downloads_pause":
        progress.paused = true;
        channel?.onmessage({ ...progress });
        return null;
      case "import_url":
        return { kind: "album", title: albums[2].title, trackCount: 10 };
      case "downloads_failed":
        return failed;
      case "download_retry":
        failed = a.jobId === "all-failed" ? [] : failed.filter((j) => j.id !== a.jobId);
        return null;
      case "downloads_subscribe":
        channel = a.channel as Channel<DownloadsProgress>;
        return null;
      case "home_get":
        return homePage;
      case "home_shelves":
        return new Promise((r) => setTimeout(() => r(account.connected ? homeShelves : []), 300));
      case "net_status":
        return true;
      case "player_subscribe": {
        const ch = a.channel as Channel<PlaybackState>;
        if (!scene) return null;
        setTimeout(() => {
          ch.onmessage({ ...state, timestampMs: Date.now() });
          void emit("track:changed", { track: NOW, palette: paletteOf(NOW.coverKey!) });
        });
        return null;
      }
      case "track_get":
        return allTracks.find((t) => t.id === a.id) ?? null;
      case "queue_get":
        return scene ? queue : { items: [], context: { kind: "none" } };
      case "lyrics_get":
        return a.trackId === NOW.id ? lyrics : null;
      case "album_get": {
        const i = albums.findIndex((x) => x.id === a.id);
        if (i < 0) throw { kind: "notFound", message: "mock: no such album" };
        return { album: albums[i], tracks: albumTracks(i) };
      }
      case "library_view": {
        const id = views.size + 1;
        const rows = view(a.kind as ViewKind);
        views.set(id, rows);
        return { viewId: id, count: rows.length };
      }
      case "library_rows": {
        const rows = views.get(a.viewId as number) ?? [];
        const kind = rows === albums ? "albums" : (rows[0] as { imageKey?: string })?.imageKey ? "artists" : "tracks";
        return { kind, items: rows.slice(a.offset as number, (a.offset as number) + (a.limit as number)) };
      }
      case "stats_get":
        return demoStats(a.period as string);
      case "integrations_status":
        return { lastfmAvailable: true, lastfmUser: account.connected ? "demo" : undefined, lastfmPending: false, discordAvailable: true };
      case "storage_get":
        return { cacheBytes: 1_830_000_000, cacheLimit: 5 * 1024 ** 3, autosavePlays: 3, dataDir: "C:\\Users\\demo\\AppData\\Local\\Mixtape" };
    }
    throw { kind: "internal", message: `mock: no command ${cmd}` };
  },
  { shouldMockEvents: true },
);

// Open the requested screen before the first frame.
switch (scene) {
  case "album":
    router.current = { name: "album", id: albums[0].id };
    break;
  case "library":
    router.current = { name: "library", tab: "albums" };
    break;
  case "stats":
    router.current = { name: "stats" };
    break;
  case "queue":
    layout.drawer = "queue";
    break;
  case "fullscreen":
    layout.fullscreen = true;
    layout.lyrics = true;
    break;
}
