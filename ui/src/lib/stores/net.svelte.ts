// Network status from the core (`net:status`): the catalog and mixes need it, the
// library never does.

import { invoke, on } from "$lib/ipc";
import { toasts } from "./overlay.svelte";
import { t } from "$lib/i18n";

class NetStore {
  online = $state(true);

  async init() {
    this.online = await invoke<boolean>("net_status").catch(() => true);
    await on<{ online: boolean }>("net:status", ({ online }) => {
      this.online = online;
      if (online) toasts.push(t.net.online);
      else toasts.push(t.net.offline, "error", 6000);
    }).catch(() => {});
  }
}

export const net = new NetStore();
