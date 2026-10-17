// Account status and download progress mirrored from the core (US2). Subscribes once at
// startup; without the core (plain browser, commands not built yet) everything stays in
// the "not connected / idle" state instead of throwing.

import { Channel, on } from "$lib/ipc";
import {
  accountConnectCookies,
  accountDisconnect,
  accountStatus,
  downloadRetry,
  downloadsFailed,
  downloadsPause,
  downloadsResume,
  downloadsSubscribe,
  type AccountStatus,
  type DownloadsProgress,
  type FailedJob,
} from "$lib/ipc/sync";
import { toasts } from "./overlay.svelte";

const idle: DownloadsProgress = { running: false, paused: false, done: 0, total: 0, failed: 0, bytesPerSec: 0 };

class Downloads {
  account = $state<AccountStatus>({ connected: false });
  progress = $state.raw<DownloadsProgress>(idle);
  failed = $state.raw<FailedJob[]>([]);
  /** False until the core answers `account_status`. */
  ready = $state(false);
  /** Global modals: account connect and «Добавить по ссылке» (`importUrl` = prefilled link). */
  connectOpen = $state(false);
  importUrl = $state<string | null>(null);

  #started = false;

  /** Downloads are moving right now (not paused). */
  get active() {
    return this.progress.running && !this.progress.paused;
  }

  async init() {
    if (this.#started) return;
    this.#started = true;
    try {
      this.account = await accountStatus();
      this.ready = true;
    } catch (e) {
      console.warn("account_status unavailable", e);
      return;
    }
    await on("account:expired", () => {
      this.account = { ...this.account, connected: false };
      toasts.push("Сессия YouTube Music истекла — загрузки остановлены", "error", 10000, {
        label: "Подключить",
        run: () => (this.connectOpen = true),
      });
    }).catch(() => {});
    try {
      const channel = new Channel<DownloadsProgress>();
      channel.onmessage = (p) => {
        const failedChanged = p.failed !== this.progress.failed;
        const finished = this.progress.running && !p.running;
        this.progress = p;
        if (failedChanged) void this.refreshFailed();
        if (finished) void this.refreshAccount(); // picks up the new `lastSyncAt`
      };
      await downloadsSubscribe(channel);
    } catch (e) {
      console.warn("downloads_subscribe unavailable", e);
    }
    void this.refreshFailed();
  }

  async refreshAccount() {
    try {
      this.account = await accountStatus();
    } catch {
      /* keeps the last known status */
    }
  }

  /** Throws `IpcError` with a readable message when the cookies are rejected. */
  async connect(cookiesTxt: string) {
    this.account = await accountConnectCookies(cookiesTxt);
    this.ready = true;
  }

  async disconnect() {
    await accountDisconnect();
    this.account = { connected: false };
  }

  async refreshFailed() {
    try {
      this.failed = await downloadsFailed();
    } catch {
      this.failed = [];
    }
  }

  pause() {
    this.progress = { ...this.progress, paused: true };
    return downloadsPause();
  }

  resume() {
    this.progress = { ...this.progress, paused: false };
    return downloadsResume();
  }

  /** Retries one failed job, or all of them. The list updates right away. */
  async retry(jobId: number | "all-failed") {
    this.failed = jobId === "all-failed" ? [] : this.failed.filter((j) => j.id !== jobId);
    try {
      await downloadRetry(jobId);
    } finally {
      void this.refreshFailed();
    }
  }
}

export const downloads = new Downloads();
