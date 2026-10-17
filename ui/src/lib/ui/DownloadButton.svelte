<script lang="ts">
  // «Скачать альбом» / «Скачать плейлист» / «Скачать дискографию» for catalog collections.
  import { invoke } from "$lib/ipc";
  import type { DownloadKind } from "$lib/ipc/bindings/DownloadKind";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import IconDownload from "~icons/solar/download-minimalistic-linear";

  let { kind, id, label }: { kind: DownloadKind; id: string; label: string } = $props();
  let busy = $state(false);

  async function download() {
    busy = true;
    try {
      await invoke("catalog_download", { kind, id });
      toasts.push(kind === "artist" ? t.downloadButton.discography : t.downloadButton.adding);
    } catch (e) {
      toasts.push(e instanceof Error ? e.message : t.trackMenu.failed, "error");
    } finally {
      busy = false;
    }
  }
</script>

<button
  type="button"
  disabled={busy}
  class="flex h-10 items-center gap-2 rounded-full bg-fill-2 px-4 text-[13px] font-medium backdrop-blur hover:bg-fill-3 disabled:opacity-50"
  onclick={download}
>
  <IconDownload class="size-4" />{label}
</button>
