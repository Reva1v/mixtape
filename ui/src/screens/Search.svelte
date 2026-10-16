<script lang="ts">
  import { invoke } from "$lib/ipc";
  import type { SearchResult } from "$lib/ipc/bindings/SearchResult";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import TrackList from "$lib/ui/TrackList.svelte";
  import Card from "$lib/ui/Card.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import IconSearch from "~icons/lucide/search";
  import IconNothing from "~icons/lucide/search-x";

  let { query = "" }: { query?: string } = $props();

  let result = $state.raw<SearchResult | null>(null);
  const tracks = new Fixed<Track>([]);
  let seq = 0;

  $effect(() => {
    const q = query.trim();
    if (!q) {
      result = null;
      return;
    }
    const mine = ++seq;
    const timer = setTimeout(async () => {
      const r = await invoke<SearchResult>("library_search", { query: q, limit: 30 });
      if (mine !== seq) return;
      result = r;
      tracks.items = r.tracks;
    }, 60);
    return () => clearTimeout(timer);
  });

  const empty = $derived(
    !!result && !result.tracks.length && !result.albums.length && !result.artists.length && !result.playlists.length,
  );

  function playTrack(i: number) {
    if (!result) return;
    void player.play({ kind: "tracks", trackIds: result.tracks.map((t) => t.id) }, i);
  }
</script>

<div class="pt-6 pb-10">
  {#if !query.trim()}
    <EmptyState icon={IconSearch} title="Поиск по библиотеке" hint="Начните вводить название трека, альбома или исполнителя" />
  {:else if empty}
    <EmptyState icon={IconNothing} title="Ничего не найдено" hint={`По запросу «${query}» в библиотеке ничего нет`} />
  {:else if result}
    <div class="space-y-8">
      {#if result.tracks.length}
        <section>
          <h2 class="px-7 pb-2 text-[20px] font-bold">Треки</h2>
          <TrackList rows={tracks} onplay={playTrack} />
        </section>
      {/if}
      {#if result.artists.length}
        <section class="px-7">
          <h2 class="pb-4 text-[20px] font-bold">Исполнители</h2>
          <div class="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-5">
            {#each result.artists.slice(0, 12) as a (a.id)}
              <Card round coverKey={a.imageKey} title={a.name} onopen={() => router.go({ name: "artist", id: a.id })} />
            {/each}
          </div>
        </section>
      {/if}
      {#if result.albums.length}
        <section class="px-7">
          <h2 class="pb-4 text-[20px] font-bold">Альбомы</h2>
          <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
            {#each result.albums.slice(0, 12) as a (a.id)}
              <Card
                coverKey={a.coverKey}
                title={a.title}
                subtitle={a.artists.map((x) => x.name).join(", ")}
                onopen={() => router.go({ name: "album", id: a.id })}
                onplay={() => player.play({ kind: "album", id: a.id })}
              />
            {/each}
          </div>
        </section>
      {/if}
      {#if result.playlists.length}
        <section class="px-7">
          <h2 class="pb-4 text-[20px] font-bold">Плейлисты</h2>
          <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
            {#each result.playlists.slice(0, 12) as p (p.id)}
              <Card
                coverKey={p.coverKey}
                title={p.title}
                subtitle={`${p.trackCount} треков`}
                onopen={() => router.go({ name: "playlist", id: p.id })}
                onplay={() => player.play({ kind: "playlist", id: p.id })}
              />
            {/each}
          </div>
        </section>
      {/if}
    </div>
  {/if}
</div>
