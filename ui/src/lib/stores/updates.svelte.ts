// New versions from GitHub Releases: the core checks on its own and announces what it
// finds; Settings can check by hand and install.

import { invoke, on } from "$lib/ipc";
import type { UpdateInfo } from "$lib/ipc/bindings/UpdateInfo";
import type { YtdlpChange } from "$lib/ipc/bindings/YtdlpChange";
import type { YtdlpInfo } from "$lib/ipc/bindings/YtdlpInfo";
import type { YtdlpStatus } from "$lib/ipc/bindings/YtdlpStatus";
import { locale, t } from "$lib/i18n";
import { toasts } from "./overlay.svelte";
import { shouldAnnounce } from "./ytdlpNotice";

type Status =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "available"; info: UpdateInfo }
  | { kind: "installing"; done: number; total: number | null }
  | { kind: "error"; message: string };

class Updates {
  version = $state("");
  status = $state<Status>({ kind: "idle" });
  /** The yt-dlp in use and what the last check found (Settings → Updates). */
  ytdlp = $state<YtdlpStatus | null>(null);
  ytdlpBusy = $state<"checking" | "reverting" | null>(null);

  async init() {
    this.version = await invoke<string>("app_version").catch(() => "");
    await on<UpdateInfo>("update:available", (info) => {
      if (this.status.kind === "installing") return;
      this.status = { kind: "available", info };
      toasts.push(t.updates.available(info.version), "info", 15000, {
        label: t.updates.install,
        run: () => void this.install(),
      });
    });
    await on<{ done: number; total: number | null }>("update:progress", (p) => {
      this.status = { kind: "installing", done: p.done, total: p.total };
    });
    await this.initYtdlp();
  }

  /** yt-dlp is never installed without the owner knowing: each version is announced once. */
  async initYtdlp() {
    this.ytdlp = await invoke<YtdlpStatus>("ytdlp_status").catch(() => null);
    await on<YtdlpInfo>("ytdlp:available", (info) => {
      void this.refreshYtdlp();
      if (!shouldAnnounce(localStorage, "available", info.latest)) return;
      const text =
        info.installAfter === undefined
          ? t.updates.ytdlp.toastManual(info.latest, info.installed)
          : t.updates.ytdlp.toastPending(info.latest, info.installed, this.date(info.installAfter));
      toasts.push(text, "info", 20000);
    });
    await on<YtdlpChange>("ytdlp:installed", (c) => {
      void this.refreshYtdlp();
      if (shouldAnnounce(localStorage, "installed", c.version)) toasts.push(t.updates.ytdlp.toastInstalled(c.version), "info", 12000);
    });
    await on<YtdlpChange>("ytdlp:rolledback", (c) => {
      void this.refreshYtdlp();
      const bad = c.bad ?? "";
      if (shouldAnnounce(localStorage, "rolledback", bad)) toasts.push(t.updates.ytdlp.toastRolledBack(bad, c.version), "error", 20000);
    });
  }

  /** A date for the interface, from Unix seconds. */
  date(unixSeconds: number) {
    return new Date(unixSeconds * 1000).toLocaleDateString(locale.tag, { day: "numeric", month: "long" });
  }

  async refreshYtdlp() {
    this.ytdlp = await invoke<YtdlpStatus>("ytdlp_status").catch(() => this.ytdlp);
  }

  async checkYtdlp() {
    this.ytdlpBusy = "checking";
    try {
      this.ytdlp = await invoke<YtdlpStatus>("ytdlp_check");
    } catch {
      /* offline: the status stays as it was */
    } finally {
      this.ytdlpBusy = null;
    }
  }

  async revertYtdlp() {
    this.ytdlpBusy = "reverting";
    try {
      this.ytdlp = await invoke<YtdlpStatus>("ytdlp_revert");
    } catch {
      /* nothing to revert */
    } finally {
      this.ytdlpBusy = null;
    }
  }

  async check() {
    this.status = { kind: "checking" };
    try {
      const info = await invoke<UpdateInfo | null>("update_check");
      this.status = info ? { kind: "available", info } : { kind: "latest" };
    } catch (e) {
      this.status = { kind: "error", message: e instanceof Error ? e.message : String(e) };
    }
  }

  async install() {
    this.status = { kind: "installing", done: 0, total: null };
    try {
      await invoke("update_install");
    } catch (e) {
      this.status = { kind: "error", message: e instanceof Error ? e.message : String(e) };
    }
  }
}

export const updates = new Updates();
