<script lang="ts">
  // «Добавить по ссылке» (T067): a track, album or playlist from YouTube / YouTube Music
  // goes to the download queue. Opened by Ctrl+L, from the downloads screen, or by
  // pasting a recognised link anywhere outside a text field.
  import { importUrl, parseMusicUrl, type ImportKind } from "$lib/ipc/sync";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import Modal from "./Modal.svelte";

  const kinds: Record<ImportKind, string> = { track: "Трек", album: "Альбом", playlist: "Плейлист", artist: "Дискография" };

  let value = $state("");
  let error = $state("");
  let busy = $state(false);
  let input = $state<HTMLInputElement>();

  const open = $derived(downloads.importUrl !== null);
  const parsed = $derived(parseMusicUrl(value));

  $effect(() => {
    if (downloads.importUrl === null) return;
    value = downloads.importUrl;
    error = "";
    busy = false;
    queueMicrotask(() => input?.select());
  });

  function close() {
    downloads.importUrl = null;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!parsed || busy) return;
    busy = true;
    error = "";
    try {
      const r = await importUrl(parsed.url);
      close();
      toasts.push(
        r.kind === "track" ? `«${r.title}» скачивается` : `${kinds[r.kind]} «${r.title}»: ${t.count.tracks(r.trackCount)} в загрузке`,
      );
    } catch (err) {
      error = err instanceof Error && err.message ? err.message : "Не удалось добавить ссылку";
    } finally {
      busy = false;
    }
  }

  // Ctrl+V outside text fields with a YouTube link opens the dialog prefilled.
  function onPaste(e: ClipboardEvent) {
    const el = e.target as HTMLElement | null;
    if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)) return;
    const link = parseMusicUrl(e.clipboardData?.getData("text") ?? "");
    if (!link) return;
    e.preventDefault();
    downloads.importUrl = link.url;
  }
</script>

<svelte:document onpaste={onPaste} />

<Modal {open} title="Добавить по ссылке" onclose={close} width={480}>
  <p class="mb-3 text-[13px] leading-snug text-ink-3">
    Трек, альбом, плейлист или исполнитель с YouTube Music или YouTube — всё скачается в библиотеку.
  </p>
  <form onsubmit={submit}>
    <input
      bind:this={input}
      bind:value
      spellcheck="false"
      placeholder="https://music.youtube.com/…"
      aria-label="Ссылка"
      class="h-10 w-full rounded-md bg-fill-2 px-3 text-[14px] outline-none ring-1 ring-line placeholder:text-ink-3 focus:ring-white/40"
    />
    <p class="mt-2 h-[18px] text-[13px] leading-[18px] {error || (value.trim() && !parsed) ? 'text-danger' : 'text-ink-3'}">
      {#if error}
        {error}
      {:else if parsed}
        {kinds[parsed.kind]}
      {:else if value.trim()}
        Это не похоже на ссылку YouTube Music или YouTube
      {/if}
    </p>
    <div class="mt-4 flex justify-end gap-2">
      <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={close}>
        {t.common.cancel}
      </button>
      <button
        type="submit"
        disabled={!parsed || busy}
        class="h-9 rounded-full bg-ink px-5 text-[13px] font-semibold text-black transition-[background-color,opacity] hover:bg-[#dcdcdc] disabled:opacity-40 disabled:hover:bg-ink"
      >
        {busy ? "Добавляем…" : t.track.download}
      </button>
    </div>
  </form>
</Modal>
