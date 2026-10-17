<script lang="ts" module>
  // Which tab the search was on last time.
  let lastScope: "library" | "catalog" = "library";
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import { invoke } from "$lib/ipc";
  import CatalogResults from "$lib/ui/CatalogResults.svelte";
  import type { SearchResult } from "$lib/ipc/bindings/SearchResult";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { openAlbum } from "$lib/stores/actions";
  import TrackList from "$lib/ui/TrackList.svelte";
  import Card from "$lib/ui/Card.svelte";
  import { albumMenu, artistMenu, playlistMenu } from "$lib/ui/collectionMenu";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { t, tracks as tracksLabel } from "$lib/i18n/ru";
  import IconSearch from "~icons/solar/magnifier-linear";
  import IconNothing from "~icons/solar/magnifier-bug-linear";
  import IconX from "~icons/solar/close-circle-linear";
  import Cover from "$lib/ui/Cover.svelte";
  import { recentSearch, type RecentItem } from "$lib/stores/recentSearch.svelte";
  import type { Album } from "$lib/ipc/bindings/Album";
  import type { Artist } from "$lib/ipc/bindings/Artist";
  import type { Playlist } from "$lib/ipc/bindings/Playlist";

  let { query = "" }: { query?: string } = $props();
  let scope = $state(lastScope);
  $effect(() => {
    lastScope = scope;
  });

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
    if (result.tracks[i]) recentSearch.add(trackItem(result.tracks[i]));
    void player.play({ kind: "tracks", trackIds: result.tracks.map((t) => t.id) }, i);
  }

  // Recent: what was opened or played from the results, and queries confirmed with Enter.
  const names = (a: { name: string }[]) => a.map((x) => x.name).join(", ");
  const trackItem = (t: Track): RecentItem => ({ kind: "track", id: t.id, title: t.title, subtitle: `Трек · ${names(t.artists)}`, coverKey: t.coverKey });
  const albumItem = (a: Album): RecentItem => ({ kind: "album", id: a.id, title: a.title, subtitle: `Альбом · ${names(a.artists)}`, coverKey: a.coverKey });
  const artistItem = (a: Artist): RecentItem => ({ kind: "artist", id: a.id, title: a.name, subtitle: "Исполнитель", coverKey: a.imageKey });
  const playlistItem = (p: Playlist): RecentItem => ({ kind: "playlist", id: p.id, title: p.title, subtitle: "Плейлист", coverKey: p.coverKey });

  function openRecent(item: RecentItem) {
    recentSearch.add(item);
    if (item.kind === "query") {
      text = item.text;
      router.replace({ name: "search", query: item.text });
      input?.focus();
    } else if (item.kind === "track") void player.play({ kind: "tracks", trackIds: [item.id] });
    else router.go({ name: item.kind, id: item.id });
  }
</script>

{#snippet recent(fallback: Snippet)}
  {#if recentSearch.items.length}
    <section class="px-7">
      <div class="flex items-baseline justify-between px-1 pb-3">
        <h2 class="text-[20px] font-bold">Недавнее</h2>
        <button type="button" class="text-[13px] font-medium text-ink-3 hover:text-ink" onclick={() => recentSearch.clear()}>Очистить</button>
      </div>
      <div class="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-x-4 gap-y-1">
        {#each recentSearch.items as item (item.kind === "query" ? `q:${item.text}` : `${item.kind}:${item.id}`)}
          <div
            role="button"
            tabindex="0"
            class="group flex h-14 cursor-pointer items-center gap-3 rounded-md px-2 transition-colors hover:bg-fill-2"
            onclick={() => openRecent(item)}
            onkeydown={(e) => e.key === "Enter" && openRecent(item)}
          >
            {#if item.kind === "query"}
              <span class="grid size-10 shrink-0 place-items-center rounded-md bg-fill-1 text-ink-3 ring-1 ring-line"><IconSearch class="size-4" /></span>
            {:else}
              <Cover coverKey={item.coverKey} size={40} rounded={item.kind === "artist" ? "full" : "sm"} />
            {/if}
            <div class="min-w-0 flex-1">
              <div class="truncate text-[13.5px] text-ink">{item.kind === "query" ? item.text : item.title}</div>
              <div class="truncate text-[12px] text-ink-3">{item.kind === "query" ? "Запрос" : item.subtitle}</div>
            </div>
            <button
              type="button"
              aria-label="Убрать из недавнего"
              class="grid size-8 shrink-0 place-items-center rounded-full text-ink-3 opacity-0 transition-opacity group-hover:opacity-100 hover:text-ink"
              onclick={(e) => {
                e.stopPropagation();
                recentSearch.remove(item);
              }}
            >
              <IconX class="size-4" />
            </button>
          </div>
        {/each}
      </div>
    </section>
  {:else}
    {@render fallback()}
  {/if}
{/snippet}

{#snippet catalogHint()}
  <EmptyState icon={IconSearch} title="Поиск в YouTube Music" hint="Треки, альбомы и исполнители за пределами библиотеки — слушайте сразу, скачивайте что понравится" />
{/snippet}
{#snippet libraryHint()}
  <EmptyState icon={IconSearch} title="Поиск по библиотеке" hint="Начните вводить название трека, альбома или исполнителя" />
{/snippet}

<div class="pb-10">
  <div class="px-8 pt-10 pb-4">
    <label class="flex h-12 items-center gap-3 rounded-xl bg-fill-1 px-4 ring-1 ring-line transition-colors focus-within:bg-fill-2 focus-within:ring-white/15">
      <IconSearch class="size-5 shrink-0 text-ink-3" />
      <input
        bind:this={input}
        bind:value={text}
        oninput={onInput}
        onkeydown={(e) => e.key === "Enter" && recentSearch.add({ kind: "query", text: text.trim() })}
        placeholder={t.search.placeholder}
        spellcheck="false"
        class="min-w-0 flex-1 bg-transparent text-[16px] text-ink outline-none placeholder:text-ink-3"
      />
    </label>
  </div>
  <div class="flex gap-6 px-8 pb-4">
    {#each [["library", "Библиотека"], ["catalog", "YouTube Music"]] as [id, label] (id)}
      <button
        type="button"
        class="relative pb-1.5 text-[14.5px] font-semibold transition-colors {scope === id ? 'text-ink' : 'text-ink-3 hover:text-ink-2'}"
        onclick={() => (scope = id as typeof scope)}
      >
        {label}
        {#if scope === id}<span class="absolute inset-x-0 -bottom-px h-0.5 rounded-full bg-ink"></span>{/if}
      </button>
    {/each}
  </div>
  {#if scope === "catalog"}
    {#if query.trim()}
      <CatalogResults {query} onpick={(item) => recentSearch.add(item)} />
    {:else}
      {@render recent(catalogHint)}
    {/if}
  {:else if !query.trim()}
    {@render recent(libraryHint)}
  {:else if empty}
    <EmptyState icon={IconNothing} title="Ничего не найдено" hint={`По запросу «${query}» в библиотеке ничего нет`}>
      <button type="button" class="h-9 rounded-full bg-ink px-4 text-[13px] font-semibold text-black" onclick={() => (scope = "catalog")}>
        Искать в YouTube Music
      </button>
    </EmptyState>
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
              <Card round coverKey={a.imageKey} title={a.name} onopen={() => {
                  recentSearch.add(artistItem(a));
                  router.go({ name: "artist", id: a.id });
                }} menu={() => artistMenu(a)} />
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
                onopen={() => {
                  recentSearch.add(albumItem(a));
                  openAlbum(a);
                }}
                onplay={() => {
                  recentSearch.add(albumItem(a));
                  void player.play({ kind: "album", id: a.id });
                }}
                menu={() => albumMenu(a)}
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
                onopen={() => {
                  recentSearch.add(playlistItem(p));
                  router.go({ name: "playlist", id: p.id });
                }}
                onplay={() => {
                  recentSearch.add(playlistItem(p));
                  void player.play({ kind: "playlist", id: p.id });
                }}
                menu={() => playlistMenu(p)}
              />
            {/each}
          </div>
        </section>
      {/if}
    </div>
  {/if}
</div>
