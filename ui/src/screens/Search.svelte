<script lang="ts">
  import { invoke } from "$lib/ipc";
  import type { SearchResult } from "$lib/ipc/bindings/SearchResult";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { openAlbum } from "$lib/stores/actions";
  import TrackList from "$lib/ui/TrackList.svelte";
  import Card from "$lib/ui/Card.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { t, tracks as tracksLabel } from "$lib/i18n/ru";
  import IconSearch from "~icons/solar/magnifier-linear";
  import IconNothing from "~icons/solar/magnifier-bug-linear";

  let { query = "" }: { query?: string } = $props();

  let input = $state<HTMLInputElement>();
  // The route keeps the query; the input only needs it once on open.
  // svelte-ignore state_referenced_locally
  let text = $state(query);

  // Focus on open and whenever Ctrl+K / "/" asks for it.
  $effect(() => {
    void layout.searchFocus;
    input?.focus();
    input?.select();
  });

  function onInput() {
    router.replace({ name: "search", query: text });
  }

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

<div class="pb-10">
  <div class="px-8 pt-10 pb-4">
    <label class="flex h-12 items-center gap-3 rounded-xl bg-fill-1 px-4 ring-1 ring-line transition-colors focus-within:bg-fill-2 focus-within:ring-white/15">
      <IconSearch class="size-5 shrink-0 text-ink-3" />
      <input
        bind:this={input}
        bind:value={text}
        oninput={onInput}
        placeholder={t.search.placeholder}
        spellcheck="false"
        class="min-w-0 flex-1 bg-transparent text-[16px] text-ink outline-none placeholder:text-ink-3"
      />
    </label>
  </div>
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
                artists={a.artists}
                onopen={() => openAlbum(a)}
                onplay={() => player.play({ kind: "album", id: a.id })}
                current={!!a && player.track?.album?.id === a.id}
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
                subtitle={tracksLabel(p.trackCount)}
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
