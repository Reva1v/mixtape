// Account, sync and downloads (US2, contracts/ipc.md → «Синхронизация и загрузки»).
// TODO: hand-written to match the contract; replace with ts-rs bindings once the Rust
// commands exist (src-tauri/src/commands/sync.rs).

import { Channel, invoke } from "./index";
import type { Playlist } from "./bindings/Playlist";
import type { TrackId } from "./bindings/TrackId";

export type AccountStatus = {
  connected: boolean;
  name?: string;
  /** Cover key of the avatar, served by `cover.localhost` (the CSP blocks remote images). */
  avatar?: string;
  /** Unix time in ms. */
  lastSyncAt?: number;
};

export type SyncPreview = { liked: number; playlists: Playlist[]; albums: number; artists: number };

export type SyncSelection = { liked: boolean; playlistIds: string[]; albums: boolean; artists: boolean };

export type ImportKind = "track" | "album" | "playlist";
export type ImportResult = { kind: ImportKind; title: string; trackCount: number };

export type DownloadsProgress = {
  running: boolean;
  paused: boolean;
  done: number;
  total: number;
  failed: number;
  current?: { trackId: TrackId; title: string; bytesDone: number; bytesTotal?: number };
  bytesPerSec: number;
};

/** A download that gave up; `error` is a Russian, human-readable reason. */
export type FailedJob = { id: number; trackId: TrackId; title: string; artists: string; error: string; attempts: number };

export const accountStatus = () => invoke<AccountStatus>("account_status");
/** Accepts Netscape cookies.txt text or an EditThisCookie JSON export. */
export const accountConnectCookies = (cookiesTxt: string) => invoke<AccountStatus>("account_connect_cookies", { cookiesTxt });
/** Opens Google sign-in in a Mixtape window; resolves once the account is connected. */
export const accountConnectWebview = () => invoke<AccountStatus>("account_connect_webview");
export const accountDisconnect = () => invoke<void>("account_disconnect");

export const syncPreview = () => invoke<SyncPreview>("sync_preview");
export const syncStart = (sel: SyncSelection) => invoke<void>("sync_start", sel);
export const importUrl = (url: string) => invoke<ImportResult>("import_url", { url });

export const downloadsPause = () => invoke<void>("downloads_pause");
export const downloadsResume = () => invoke<void>("downloads_resume");
export const downloadRetry = (jobId: number | "all-failed") => invoke<void>("download_retry", { jobId });
export const downloadsFailed = () => invoke<FailedJob[]>("downloads_failed");
/** Progress summary at ≤4 Hz. */
export const downloadsSubscribe = (channel: Channel<DownloadsProgress>) => invoke<void>("downloads_subscribe", { channel });

/** Recognises a YouTube / YouTube Music link to a track, album or playlist. */
export function parseMusicUrl(text: string): { url: string; kind: ImportKind } | null {
  let u: URL;
  try {
    u = new URL(text.trim());
  } catch {
    return null;
  }
  if (u.protocol !== "https:" && u.protocol !== "http:") return null;
  const host = u.hostname.replace(/^(www|m)\./, "");
  const list = u.searchParams.get("list");
  if (host === "youtu.be") return /^\/[\w-]{11}$/.test(u.pathname) ? { url: u.href, kind: "track" } : null;
  if (host !== "youtube.com" && host !== "music.youtube.com") return null;
  // Albums are playlists with an `OLAK5uy_` id, or `browse/MPREb_…` pages on YTM.
  if (u.pathname.startsWith("/browse/MPREb")) return { url: u.href, kind: "album" };
  if (u.pathname === "/watch" && u.searchParams.get("v")) return { url: u.href, kind: "track" };
  if (/^\/shorts\/[\w-]{11}$/.test(u.pathname)) return { url: u.href, kind: "track" };
  if (u.pathname === "/playlist" && list) return { url: u.href, kind: list.startsWith("OLAK5uy_") ? "album" : "playlist" };
  return null;
}
