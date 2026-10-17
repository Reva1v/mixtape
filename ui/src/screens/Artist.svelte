<script lang="ts">
  import { invoke, on } from "$lib/ipc";
  import type { ArtistPage } from "$lib/ipc/bindings/ArtistPage";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { openAlbum } from "$lib/stores/actions";
  import { tracks } from "$lib/i18n/ru";
  import CollectionHeader from "$lib/ui/CollectionHeader.svelte";
  import TrackList from "$lib/ui/TrackList.svelte";
  import Card from "$lib/ui/Card.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import Skeleton from "$lib/ui/Skeleton.svelte";
  import IconAlert from "~icons/solar/danger-circle-linear";

  let { id }: { id: string } = $props();

  let page = $state.raw<ArtistPage | null>(null);
  let error = $state<string | null>(null);
  let showAll = $state(false);
  const rows = new Fixed<Track>([]);

  async function load() {
    try {
      page = await invoke<ArtistPage>("artist_get", { id });
    } catch (e) {
      error = (e as Error).message;
    }
  }

  $effect(() => {
    void load();
    const un = on("library:changed", () => void load());
    return () => void un.then((f) => f());
  });

  $effect(() => {
    if (page) rows.items = showAll ? page.tracks : page.tracks.slice(0, 5);
  });

  function play(i = 0, shuffle = false) {
    if (shuffle !== player.state.shuffle) void player.toggleShuffle();
    void player.play({ kind: "artist", id }, i);
  }
</script>

{#if error}
  <EmptyState icon={IconAlert} title="Не удалось открыть исполнителя" hint={error} />
{:else if !page}
  <div class="flex items-end gap-7 px-7 pt-10">
    <Skeleton class="size-52 rounded-full" />
    <div class="flex-1 space-y-3"><Skeleton class="h-10 w-1/2" /></div>
  </div>
{:else}
  <CollectionHeader
    kind="Исполнитель"
    title={page.artist.name}
    coverKey={page.artist.imageKey}
    round
    onplay={page.tracks.length ? () => play(0) : undefined}
    current={page.tracks.some((x) => x.id === player.currentId)}
    onshuffle={page.tracks.length ? () => play(0, true) : undefined}
    onmix={page.tracks[0] ? () => player.mix(page!.tracks[0].id) : undefined}
  >
    {#snippet meta()}
      <span>{tracks(page!.tracks.length)} в библиотеке</span>
    {/snippet}
  </CollectionHeader>

  <div class="space-y-8 pb-10">
    {#if page.tracks.length}
      <section>
        <h2 class="px-7 pb-2 text-[20px] font-bold">Популярное у вас</h2>
        <TrackList {rows} onplay={(i) => play(i)} />
        {#if page.tracks.length > 5}
          <button type="button" class="mt-2 ml-7 text-[13px] font-medium text-ink-2 hover:text-ink" onclick={() => (showAll = !showAll)}>
            {showAll ? "Свернуть" : `Показать все (${page.tracks.length})`}
          </button>
        {/if}
      </section>
    {/if}
    {#if page.albums.length}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">Альбомы</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
          {#each page.albums as a (a.id)}
            <Card
              coverKey={a.coverKey}
              title={a.title}
              subtitle={[a.year, { album: "Альбом", ep: "EP", single: "Сингл" }[a.albumType]].filter(Boolean).join(" • ")}
              onopen={() => openAlbum(a)}
              onplay={() => player.play({ kind: "album", id: a.id })}
                current={!!a && player.track?.album?.id === a.id}
            />
          {/each}
        </div>
      </section>
    {/if}
  </div>
{/if}
