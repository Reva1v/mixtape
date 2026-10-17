// New versions from GitHub Releases: the core checks on its own and announces what it
// finds; Settings can check by hand and install.

import { invoke, on } from "$lib/ipc";
import type { UpdateInfo } from "$lib/ipc/bindings/UpdateInfo";
import { t } from "$lib/i18n";
import { toasts } from "./overlay.svelte";

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
