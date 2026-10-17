<script lang="ts">
  // Runs one Spotify / track-list import inside «Add from link»: progress while tracks are
  // looked up on YouTube Music, then the summary with what was not found. Unmounting
  // (closing the dialog) while it runs cancels the import; nothing is saved then.
  import { on } from "$lib/ipc";
  import {
    spotifyCancel,
    spotifyDownload,
    spotifyImport,
    trackLine,
    type SpotifyImportResult,
    type SpotifyProgress,
    type SpotifySource,
  } from "$lib/ipc/spotify";
  import { router } from "$lib/stores/router.svelte";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";

  let {
    source,
    onclose,
    finished = $bindable(false),
  }: { source: SpotifySource; onclose: () => void; finished?: boolean } = $props();

  let progress = $state<SpotifyProgress | null>(null);
  let result = $state.raw<SpotifyImportResult | null>(null);
  let error = $state("");
  let downloading = $state(false);

  $effect(() => {
    let live = true;
    const un = on<SpotifyProgress>("spotify:progress", (p) => {
      if (live) progress = p;
    });
    spotifyImport(source)
      .then((r) => {
        if (live) result = r;
      })
      .catch((e) => {
        if (live) error = e instanceof Error && e.message ? e.message : t.spotify.failed;
      })
      .finally(() => {
        if (live) finished = true;
      });
    return () => {
      live = false;
      void un.then((f) => f());
      if (!finished) void spotifyCancel().catch(() => {});
    };
  });

  const share = $derived(progress && progress.total > 0 ? progress.done / progress.total : 0);
  const missing = $derived(result ? result.notFound.map(trackLine).join("\n") : "");

  async function copy() {
    try {
      await navigator.clipboard.writeText(missing);
      toasts.push(t.spotify.copied);
    } catch {
      toasts.push(t.spotify.failed, "error");
    }
  }

  function open() {
    if (!result?.playlistId) return;
    router.go({ name: "playlist", id: result.playlistId });
    onclose();
  }

  async function download() {
    if (!result?.playlistId || downloading) return;
    downloading = true;
    try {
      const n = await spotifyDownload(result.playlistId);
      toasts.push(n > 0 ? t.spotify.queued(t.count.tracks(n)) : t.spotify.alreadyDownloaded);
      onclose();
    } catch (e) {
      toasts.push(e instanceof Error && e.message ? e.message : t.spotify.failed, "error");
    } finally {
      downloading = false;
    }
  }
</script>

{#if result}
  <p class="text-[20px] font-semibold leading-tight">{t.spotify.matched(result.matched, result.total)}</p>
  <p class="mt-1 text-[13px] leading-snug text-ink-3">
    {result.playlistId ? t.spotify.savedAs(result.title) : t.spotify.nothingMatched}
  </p>
  {#if result.cut}
    <p class="mt-2 text-[13px] leading-snug text-ink-2">
      {result.cut.total != null
        ? t.spotify.cutKnown(result.cut.read, t.count.tracks(result.cut.total))
        : t.spotify.cutUnknown(t.count.tracks(result.cut.read))}
    </p>
  {/if}
  {#if result.notFound.length}
    <div class="mt-4 flex items-center justify-between">
      <span class="text-[13px] font-medium text-ink-2">{t.spotify.notFound(result.notFound.length)}</span>
      <button type="button" class="h-7 rounded-full px-3 text-[12px] text-ink-2 hover:bg-fill-2" onclick={copy}>
        {t.spotify.copy}
      </button>
    </div>
    <ul class="mt-1 max-h-48 select-text overflow-y-auto rounded-md bg-fill-2 px-3 py-2 text-[13px] leading-[22px] text-ink-2 ring-1 ring-line">
      {#each result.notFound as track, i (i)}
        <li class="truncate" title={trackLine(track)}>{trackLine(track)}</li>
      {/each}
    </ul>
  {/if}
  <div class="mt-5 flex justify-end gap-2">
    <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={onclose}>
      {t.spotify.close}
    </button>
    {#if result.playlistId}
      <button
        type="button"
        disabled={downloading}
        class="h-9 rounded-full bg-fill-2 px-4 text-[13px] font-medium hover:bg-fill-3 disabled:opacity-50"
        onclick={download}
      >
        {t.spotify.download}
      </button>
      <button
        type="button"
        class="h-9 rounded-full bg-ink px-5 text-[13px] font-semibold text-black transition-colors hover:bg-[#dcdcdc]"
        onclick={open}
      >
        {t.spotify.open}
      </button>
    {/if}
  </div>
{:else if error}
  <p class="text-[13px] leading-snug text-danger">{error}</p>
  <div class="mt-5 flex justify-end">
    <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={onclose}>
      {t.spotify.close}
    </button>
  </div>
{:else}
  <p class="text-[13px] leading-snug text-ink-2">
    {progress ? t.spotify.matching(progress.done, progress.total) : source.kind === "link" ? t.spotify.reading : ""}
  </p>
  <div class="mt-3 h-1 overflow-hidden rounded-full bg-fill-2">
    <div class="h-full origin-left rounded-full bg-ink transition-transform duration-300" style="transform: scaleX({share})"></div>
  </div>
  <div class="mt-5 flex justify-end">
    <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={onclose}>
      {t.common.cancel}
    </button>
  </div>
{/if}
