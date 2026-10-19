// Account status and download progress mirrored from the core (US2). Subscribes once at
// startup; without the core (plain browser, commands not built yet) everything stays in
// the "not connected / idle" state instead of throwing.

import { Channel, on } from "$lib/ipc";
import {
  accountConnectCookies,
  accountConnectWebview,
  accountDisconnect,
  accountStatus,
  downloadRetry,
  downloadsFailed,
  downloadsPause,
  downloadsResume,
  downloadsSubscribe,
  syncPreview,
  type AccountStatus,
  type DownloadsProgress,
  type FailedJob,
  type SyncPreview,
} from "$lib/ipc/sync";
import { toasts } from "./overlay.svelte";
import { t } from "$lib/i18n";

const idle: DownloadsProgress = { running: false, paused: false, done: 0, total: 0, failed: 0, bytesPerSec: 0 };

class Downloads {
  #account = $state<AccountStatus>({ connected: false });
  /** The account the playlist picks were made for: another account's playlists are not offered. */
  #pickOwner: string | undefined;
  get account(): AccountStatus {
    return this.#account;
  }
  set account(a: AccountStatus) {
    this.#account = a;
    if (a.connected && a.name !== this.#pickOwner) {
      if (this.#pickOwner !== undefined) this.preview = null;
      this.#pickOwner = a.name;
      this.pick.playlistIds = [];
    }
  }
  progress = $state.raw<DownloadsProgress>(idle);
  failed = $state.raw<FailedJob[]>([]);
  /** False until the core answers `account_status`. */
  ready = $state(false);
  /** Global modals: account connect and «Добавить по ссылке» (`importUrl` = prefilled link). */
  connectOpen = $state(false);
  importUrl = $state<string | null>(null);

  /** What the account has to sync. Kept between visits to the Downloads screen and
   * refreshed in the background, so the list does not reload and jump every time. */
  preview = $state.raw<SyncPreview | null>(null);
  previewError = $state("");
  /** The user's picks on the Downloads screen. */
  pick = $state({ liked: true, albums: false, artists: false, playlistIds: [] as string[] });

  #started = false;
  #previewLoading = false;

  /** Downloads are moving right now (not paused). */
  get active() {
    return !!this.progress.sync || (this.progress.running && !this.progress.paused);
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
    // Sign-in finished in its own window; the call that opened it may be gone (reload).
    await on("account:changed", async () => {
      await this.refreshAccount();
      if (this.connectOpen && this.account.connected) {
        this.connectOpen = false;
        toasts.push(this.account.name ? t.connect.connected(this.account.name) : t.connect.connectedPlain);
      }
    }).catch(() => {});
    await on("account:expired", () => {
      this.account = { ...this.account, connected: false };
      toasts.push(t.account.expired, "error", 10000, {
        label: t.connect.connect,
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

  async loadPreview() {
    if (this.#previewLoading) return;
    this.#previewLoading = true;
    try {
      const next = await syncPreview();
      // Replace only on change, so the rendered list stays put.
      if (JSON.stringify(next) !== JSON.stringify(this.preview)) this.preview = next;
      this.previewError = "";
    } catch (e) {
      if (!this.preview) this.previewError = e instanceof Error && e.message ? e.message : t.account.previewFailed;
    } finally {
      this.#previewLoading = false;
    }
  }

  /** Throws `IpcError` with a readable message when the cookies are rejected. */
  async connect(cookiesTxt: string) {
    this.account = await accountConnectCookies(cookiesTxt);
    this.ready = true;
  }

  /** Sign-in window; throws `IpcError` when the window is closed or YouTube says no. */
  async signIn() {
    this.account = await accountConnectWebview();
    this.ready = true;
  }

  async disconnect() {
    await accountDisconnect();
    this.account = { connected: false };
    this.preview = null;
    this.pick.playlistIds = [];
    this.#pickOwner = undefined;
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
