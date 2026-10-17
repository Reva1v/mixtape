<script lang="ts">
  import { invoke, on } from "$lib/ipc";
  import type { AlbumPage } from "$lib/ipc/bindings/AlbumPage";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { formatTime, player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { tracks } from "$lib/i18n/ru";
  import type { Track } from "$lib/ipc/bindings/Track";
  import CollectionHeader from "$lib/ui/CollectionHeader.svelte";
  import TrackList from "$lib/ui/TrackList.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import Skeleton from "$lib/ui/Skeleton.svelte";
  import IconAlert from "~icons/solar/danger-circle-linear";
  import DownloadButton from "$lib/ui/DownloadButton.svelte";

  let { id }: { id: string } = $props();

  let page = $state.raw<AlbumPage | null>(null);
  let error = $state<string | null>(null);
  const rows = new Fixed<Track>([]);

  async function load() {
    try {
      page = await invoke<AlbumPage>("album_get", { id });
      rows.items = page.tracks;
    } catch (e) {
      error = (e as Error).message;
    }
  }

  $effect(() => {
    void load();
    const un = on("library:changed", () => void load());
    return () => void un.then((f) => f());
  });

  const total = $derived(page ? page.tracks.reduce((s, t) => s + t.durationMs, 0) : 0);
  const typeLabel = { album: "Альбом", ep: "EP", single: "Сингл" } as const;

  function play(i = 0, shuffle = false) {
    if (shuffle !== player.state.shuffle) void player.toggleShuffle();
    void player.play({ kind: "album", id }, i);
  }
</script>

{#if error}
  <EmptyState icon={IconAlert} title="Не удалось открыть альбом" hint={error} />
{:else if !page}
  <div class="flex items-end gap-7 px-7 pt-10">
    <Skeleton class="size-52 rounded-lg" />
    <div class="flex-1 space-y-3"><Skeleton class="h-10 w-1/2" /><Skeleton class="h-4 w-1/3" /></div>
  </div>
{:else}
  <CollectionHeader
    kind={typeLabel[page.album.albumType]}
    title={page.album.title}
    coverKey={page.album.coverKey}
    palette={page.album.palette}
    onplay={() => play(0)}
    current={page.tracks.some((x) => x.id === player.currentId)}
    onshuffle={() => play(0, true)}
    onmix={page.tracks[0] ? () => player.mix(page!.tracks[0].id) : undefined}
  >
    {#snippet actions()}
      {#if page!.tracks.some((t) => t.state === "remote" || t.state === "cached")}
        <DownloadButton kind="album" {id} label="Скачать альбом" />
      {/if}
    {/snippet}
    {#snippet meta()}
      {#each page!.album.artists as a, i (i)}
        {#if i > 0}<span>,</span>{/if}
        {#if a.id}
          <button type="button" class="font-semibold text-ink hover:underline" onclick={() => router.go({ name: "artist", id: a.id! })}>{a.name}</button>
        {:else}<span class="font-semibold text-ink">{a.name}</span>{/if}
      {/each}
      {#if page!.album.year}<span>• {page!.album.year}</span>{/if}
      <span>• {tracks(page!.tracks.length)}, {formatTime(total)}</span>
    {/snippet}
  </CollectionHeader>
  <div class="pb-10">
    <TrackList {rows} numbered showAlbum={false} showCover={false} onplay={(i) => play(i)} />
  </div>
{/if}
