<script lang="ts">
  // «Добавить по ссылке» (T067): a track, album or playlist from YouTube / YouTube Music
  // goes to the download queue. Opened by Ctrl+L, from the downloads screen, or by
  // pasting a recognised link anywhere outside a text field. A Spotify playlist or album
  // link (or a pasted «Artist — Title» list) is matched on YouTube Music instead and saved
  // as a local playlist (SpotifyImport.svelte).
  import { importUrl, parseMusicUrl, type ImportKind } from "$lib/ipc/sync";
  import { parseSpotifyUrl, type SpotifySource } from "$lib/ipc/spotify";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import Modal from "./Modal.svelte";
  import SpotifyImport from "./SpotifyImport.svelte";

  const kinds: Record<ImportKind, string> = $derived(t.importUrl.kind);

  let value = $state("");
  let error = $state("");
  let busy = $state(false);
  let input = $state<HTMLInputElement>();
  /** The pasted-list form instead of the link field. */
  let listMode = $state(false);
  let listText = $state("");
  let listName = $state("");
  /** A running or finished Spotify / list import. */
  let run = $state.raw<SpotifySource | null>(null);
  let runFinished = $state(false);

  const open = $derived(downloads.importUrl !== null);
  const parsed = $derived(parseMusicUrl(value));
  const spotify = $derived(parsed ? null : parseSpotifyUrl(value));
  const listLines = $derived(listText.split("\n").filter((l) => l.trim()).length);
  const title = $derived(
    run && runFinished ? t.spotify.doneTitle : listMode || run?.kind === "lines" ? t.spotify.listTitle : t.importUrl.title,
  );

  $effect(() => {
    if (downloads.importUrl === null) return;
    value = downloads.importUrl;
    error = "";
    busy = false;
    listMode = false;
    run = null;
    runFinished = false;
    queueMicrotask(() => input?.select());
  });

  function close() {
    downloads.importUrl = null;
    run = null;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (spotify) {
      startRun({ kind: "link", url: spotify.url });
      return;
    }
    if (!parsed || busy) return;
    busy = true;
    error = "";
    try {
      const r = await importUrl(parsed.url);
      close();
      toasts.push(
        r.kind === "track"
          ? t.importUrl.trackAdded(r.title)
          : t.importUrl.collectionAdded(kinds[r.kind], r.title, t.count.tracks(r.trackCount)),
      );
    } catch (err) {
      error = err instanceof Error && err.message ? err.message : t.importUrl.failed;
    } finally {
      busy = false;
    }
  }

  function submitList(e: SubmitEvent) {
    e.preventDefault();
    if (!listLines) return;
    startRun({ kind: "lines", text: listText, name: listName.trim() || t.spotify.defaultName });
  }

  function startRun(source: SpotifySource) {
    runFinished = false;
    run = source;
  }

  // Ctrl+V outside text fields with a YouTube or Spotify link opens the dialog prefilled.
  function onPaste(e: ClipboardEvent) {
    const el = e.target as HTMLElement | null;
    if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)) return;
    const text = e.clipboardData?.getData("text") ?? "";
    const link = parseMusicUrl(text) ?? parseSpotifyUrl(text);
    if (!link) return;
    e.preventDefault();
    downloads.importUrl = link.url;
  }
</script>

<svelte:document onpaste={onPaste} />

<Modal {open} {title} onclose={close} width={480}>
  {#if run}
    <SpotifyImport source={run} onclose={close} bind:finished={runFinished} />
  {:else if listMode}
    <p class="mb-3 text-[13px] leading-snug text-ink-3">{t.spotify.listIntro}</p>
    <form onsubmit={submitList}>
      <input
        bind:value={listName}
        spellcheck="false"
        placeholder={t.spotify.defaultName}
        aria-label={t.spotify.name}
        class="mb-2 h-10 w-full rounded-md bg-fill-2 px-3 text-[14px] outline-none ring-1 ring-line placeholder:text-ink-3 focus:ring-white/40"
      />
      <textarea
        bind:value={listText}
        spellcheck="false"
        placeholder={t.spotify.listPlaceholder}
        aria-label={t.spotify.listTitle}
        class="h-40 w-full resize-none rounded-md bg-fill-2 px-3 py-2.5 text-[13px] leading-relaxed text-ink outline-none ring-1 ring-line placeholder:text-ink-3 focus:ring-white/40"
      ></textarea>
      <p class="mt-2 h-[18px] text-[13px] leading-[18px] text-ink-3">
        {listLines ? t.spotify.listCount(t.count.tracks(listLines)) : ""}
      </p>
      <div class="mt-4 flex items-center gap-2">
        <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={() => (listMode = false)}>
          {t.spotify.back}
        </button>
        <span class="flex-1"></span>
        <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={close}>
          {t.common.cancel}
        </button>
        <button
          type="submit"
          disabled={!listLines}
          class="h-9 rounded-full bg-ink px-5 text-[13px] font-semibold text-black transition-[background-color,opacity] hover:bg-[#dcdcdc] disabled:opacity-40 disabled:hover:bg-ink"
        >
          {t.spotify.import}
        </button>
      </div>
    </form>
  {:else}
    <p class="mb-3 text-[13px] leading-snug text-ink-3">
      {t.importUrl.intro}
      {t.spotify.hint}
    </p>
    <form onsubmit={submit}>
      <input
        bind:this={input}
        bind:value
        spellcheck="false"
        placeholder="https://music.youtube.com/…"
        aria-label={t.importUrl.link}
        class="h-10 w-full rounded-md bg-fill-2 px-3 text-[14px] outline-none ring-1 ring-line placeholder:text-ink-3 focus:ring-white/40"
      />
      <p class="mt-2 h-[18px] text-[13px] leading-[18px] {error || (value.trim() && !parsed && !spotify) ? 'text-danger' : 'text-ink-3'}">
        {#if error}
          {error}
        {:else if parsed}
          {kinds[parsed.kind]}
        {:else if spotify}
          {t.spotify.kind[spotify.kind]}
        {:else if value.trim()}
          {t.importUrl.notALink}
        {/if}
      </p>
      <div class="mt-4 flex items-center gap-2">
        <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={() => (listMode = true)}>
          {t.spotify.pasteList}
        </button>
        <span class="flex-1"></span>
        <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={close}>
          {t.common.cancel}
        </button>
        <button
          type="submit"
          disabled={!(parsed || spotify) || busy}
          class="h-9 rounded-full bg-ink px-5 text-[13px] font-semibold text-black transition-[background-color,opacity] hover:bg-[#dcdcdc] disabled:opacity-40 disabled:hover:bg-ink"
        >
          {busy ? t.importUrl.adding : spotify ? t.spotify.import : t.track.download}
        </button>
      </div>
    </form>
  {/if}
</Modal>
