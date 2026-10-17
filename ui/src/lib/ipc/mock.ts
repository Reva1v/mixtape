// Dev-only fake core for looking at screens in a plain browser: open the Vite page with
// `?mock` (never bundled into the app, see main.ts). Only the sync/downloads commands
// answer, with made-up data; everything else fails the way a missing core does.

import { mockIPC } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { AccountStatus, DownloadsProgress, FailedJob, SyncPreview } from "./sync";

let account: AccountStatus = { connected: new URLSearchParams(location.search).get("mock") !== "off" };
const named = (): AccountStatus => ({ connected: true, name: "Тестовый пользователь", lastSyncAt: Date.now() - 3_600_000 });
if (account.connected) account = named();

const preview: SyncPreview = {
  liked: 412,
  albums: 23,
  artists: 9,
  playlists: [
    { id: "PL1", title: "Для пробежки", origin: "ytm", trackCount: 48 },
    { id: "PL2", title: "Вечерний плейлист", origin: "ytm", trackCount: 112 },
    { id: "PL3", title: "Тестовый плейлист с очень длинным названием, чтобы проверить обрезку", origin: "ytm", trackCount: 7 },
  ],
};

let failed: FailedJob[] = [
  { id: 1, trackId: "aaaaaaaaaaa", title: "Тестовый трек 1", artists: "Исполнитель А", error: "Видео недоступно в вашей стране", attempts: 3 },
  { id: 2, trackId: "bbbbbbbbbbb", title: "Тестовый трек 2", artists: "Исполнитель Б, Исполнитель В", error: "Заблокировано правообладателем", attempts: 3 },
  { id: 3, trackId: "ccccccccccc", title: "Тестовый трек 3", artists: "Исполнитель Г", error: "Нет аудиопотока 251 и 140", attempts: 1 },
];

const progress: DownloadsProgress = { running: true, paused: false, done: 57, total: 140, failed: failed.length, bytesPerSec: 0 };
let channel: Channel<DownloadsProgress> | null = null;
let bytes = 0;

setInterval(() => {
  if (!progress.running || progress.paused) return;
  const total = 5_400_000;
  bytes += 260_000 + Math.random() * 120_000;
  if (bytes >= total) {
    bytes = 0;
    progress.done = Math.min(progress.total, progress.done + 1);
  }
  progress.bytesPerSec = 1_250_000 + Math.random() * 300_000;
  progress.current = { trackId: "ddddddddddd", title: `Тестовый трек ${progress.done + 1}`, bytesDone: bytes, bytesTotal: total };
  channel?.onmessage({ ...progress, failed: failed.length });
}, 250);

mockIPC(
  (cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case "app_ready":
        return null;
      case "account_status":
        return account;
      case "account_connect_cookies":
        if (!String(a.cookiesTxt).includes("youtube")) throw { kind: "auth", message: "В тексте нет cookies для youtube.com" };
        return (account = named());
      case "account_disconnect":
        account = { connected: false };
        return null;
      case "sync_preview":
        return new Promise((r) => setTimeout(() => r(preview), 600));
      case "sync_start":
      case "downloads_resume":
        progress.running = true;
        progress.paused = false;
        return null;
      case "downloads_pause":
        progress.paused = true;
        channel?.onmessage({ ...progress });
        return null;
      case "import_url":
        return { kind: "album", title: "Тестовый альбом", trackCount: 12 };
      case "downloads_failed":
        return failed;
      case "download_retry":
        failed = a.jobId === "all-failed" ? [] : failed.filter((j) => j.id !== a.jobId);
        return null;
      case "downloads_subscribe":
        channel = a.channel as Channel<DownloadsProgress>;
        return null;
    }
    throw { kind: "internal", message: `mock: нет команды ${cmd}` };
  },
  { shouldMockEvents: true },
);
