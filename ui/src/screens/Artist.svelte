<script lang="ts" module>
  import type { ArtistPage as P } from "$lib/ipc/bindings/ArtistPage";
  import type { ArtistCatalog as C } from "$lib/ipc/bindings/ArtistCatalog";
  import type { ArtistSongs as S } from "$lib/ipc/bindings/ArtistSongs";
  // Recently shown artists: going back renders at full height at once, so the scroll
  // position is restored without a jump; fresh copies replace them quietly.
  type Memo = { page?: P; catalog?: C; songs?: S };
  const cache = new Map<string, Memo>();
  function memo(id: string): Memo {
    const m = cache.get(id) ?? {};
    cache.delete(id);
    cache.set(id, m);
    if (cache.size > 20) cache.delete(cache.keys().next().value!);
    return m;
  }
</script>

<script lang="ts">
  import { invoke, on } from "$lib/ipc";
  import type { ArtistPage } from "$lib/ipc/bindings/ArtistPage";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { glowOf, player } from "$lib/stores/player.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { openAlbum } from "$lib/stores/actions";
  import { tracks } from "$lib/i18n/ru";
  import CollectionHeader from "$lib/ui/CollectionHeader.svelte";
  import TrackList from "$lib/ui/TrackList.svelte";
  import Card from "$lib/ui/Card.svelte";
  import { albumMenu } from "$lib/ui/collectionMenu";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import Skeleton from "$lib/ui/Skeleton.svelte";
  import IconAlert from "~icons/solar/danger-circle-linear";
  import DownloadButton from "$lib/ui/DownloadButton.svelte";
  import type { ArtistCatalog } from "$lib/ipc/bindings/ArtistCatalog";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { net } from "$lib/stores/net.svelte";
  import type { ArtistSongs } from "$lib/ipc/bindings/ArtistSongs";
  import { toasts } from "$lib/stores/overlay.svelte";
  import IconSongs from "~icons/solar/music-notes-linear";
  import IconRefresh from "~icons/solar/refresh-linear";

  let { id }: { id: string } = $props();

  // svelte-ignore state_referenced_locally
  const saved = memo(id);
  let page = $state.raw<ArtistPage | null>(saved.page ?? null);
  let error = $state<string | null>(null);
  let showAll = $state(false);
  const rows = new Fixed<Track>([]);
  const topRows = new Fixed<Track>(saved.catalog?.top.slice(0, 10) ?? []);

  // What YouTube Music has beyond the library: popular tracks and the full discography.
  let catalog = $state.raw<ArtistCatalog | null>(saved.catalog ?? null);
  $effect(() => {
    const artist = id;
    if (!artist.startsWith("UC") || !downloads.account.connected || !net.online) return;
    void invoke<ArtistCatalog>("artist_catalog", { id: artist })
      .then((c) => {
        if (artist !== id) return;
        catalog = saved.catalog = c;
        topRows.items = c.top.slice(0, 10);
      })
      .catch(() => {});
  });
  const owned = $derived(new Set(page?.albums.map((a) => a.id) ?? []));
  const discography = $derived(catalog?.albums.filter((a) => !owned.has(a.id)) ?? []);
  const seed = $derived(page?.tracks[0]?.id ?? catalog?.top[0]?.id);

  // «Загрузить все песни»: every album of the discography read into the library as
  // streamable tracks (nothing is downloaded); remembered per artist.
  let songs = $state.raw<ArtistSongs | null>(saved.songs ?? null);
  const songRows = new Fixed<Track>(saved.songs?.tracks ?? []);
  let loading = $state<{ done: number; total: number } | null>(null);
  $effect(() => {
    const artist = id;
    loading = null;
    void invoke<ArtistSongs | null>("artist_songs", { id: artist, load: false }).then((s) => {
      if (artist !== id || !s) return;
      songs = saved.songs = s;
      songRows.items = s.tracks;
    });
    const un = on<{ id: string; done: number; total: number }>("artist:loading", (e) => {
      if (e.id === id) loading = { done: e.done, total: e.total };
    });
    return () => void un.then((f) => f());
  });

  async function loadSongs() {
    const artist = id;
    loading = { done: 0, total: 0 };
    try {
      const s = await invoke<ArtistSongs | null>("artist_songs", { id: artist, load: true });
      if (artist !== id || !s) return;
      songs = saved.songs = s;
      songRows.items = s.tracks;
    } catch (e) {
      toasts.push(e instanceof Error ? e.message : "Не удалось загрузить песни", "error");
    } finally {
      if (artist === id) loading = null;
    }
  }

  function playSongs(i: number) {
    if (songs) void player.play({ kind: "tracks", trackIds: songs.tracks.map((t) => t.id) }, i);
  }

  const canLoad = $derived(id.startsWith("UC") && downloads.account.connected && net.online);
  const loadLabel = $derived(
    loading ? (loading.total ? `Загружаю ${loading.done} из ${loading.total}…` : "Загружаю…") : "Загрузить все песни",
  );

  function playTop(i: number) {
    if (catalog) void player.play({ kind: "tracks", trackIds: catalog.top.map((t) => t.id) }, i);
  }

  async function load() {
    try {
      page = saved.page = await invoke<ArtistPage>("artist_get", { id });
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

  // It's the artist's page: the header takes the colours of their picture, and goes
  // back to the track's colours on leaving.
  $effect(() => {
    const palette = page?.artist.palette ?? [];
    layout.pageGlow = palette.length ? glowOf(palette) : null;
  });
  $effect(() => () => (layout.pageGlow = null));

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
    onplay={page.tracks.length || catalog?.top.length ? () => play(0) : undefined}
    current={page.tracks.some((x) => x.id === player.currentId)}
    onshuffle={page.tracks.length ? () => play(0, true) : undefined}
    onmix={seed ? () => player.mix(seed!) : undefined}
  >
    {#snippet meta()}
      <span>{tracks(page!.tracks.length)} в библиотеке</span>
    {/snippet}
    {#snippet actions()}
      {#if canLoad && !songs}
        <button
          type="button"
          disabled={!!loading}
          class="flex h-10 items-center gap-2 rounded-full bg-fill-2 px-4 text-[13px] font-medium backdrop-blur hover:bg-fill-3 disabled:opacity-60"
          onclick={loadSongs}
        >
          <IconSongs class="size-4" />{loadLabel}
        </button>
      {/if}
      {#if discography.length}
        <DownloadButton kind="artist" {id} label="Скачать дискографию" />
      {/if}
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
        <h2 class="pb-4 text-[20px] font-bold">{catalog ? "В библиотеке" : "Альбомы"}</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
          {#each page.albums as a (a.id)}
            <Card
              coverKey={a.coverKey}
              title={a.title}
              subtitle={[a.year, { album: "Альбом", ep: "EP", single: "Сингл" }[a.albumType]].filter(Boolean).join(" • ")}
              onopen={() => openAlbum(a)}
              onplay={() => player.play({ kind: "album", id: a.id })}
              menu={() => albumMenu(a)}
                current={!!a && player.track?.album?.id === a.id}
            />
          {/each}
        </div>
      </section>
    {/if}
    {#if songs}
      <section>
        <div class="flex items-baseline gap-3 px-7 pb-2">
          <h2 class="text-[20px] font-bold">Все песни</h2>
          <span class="text-[13px] text-ink-3">{tracks(songs.tracks.length)}</span>
          {#if canLoad}
            <button
              type="button"
              disabled={!!loading}
              class="ml-auto flex items-center gap-1.5 text-[13px] font-medium text-ink-3 hover:text-ink disabled:opacity-60"
              onclick={loadSongs}
            >
              <IconRefresh class="size-3.5" />{loading ? loadLabel : "Обновить"}
            </button>
          {/if}
        </div>
        <TrackList rows={songRows} onplay={playSongs} />
      </section>
    {/if}
    {#if catalog?.top.length}
      <section>
        <h2 class="px-7 pb-2 text-[20px] font-bold">Популярное в YouTube Music</h2>
        <TrackList rows={topRows} onplay={playTop} />
      </section>
    {/if}
    {#if discography.length}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">Дискография</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
          {#each discography as a (a.id)}
            <Card
              coverKey={a.coverKey}
              title={a.title}
              subtitle={[a.year, { album: "Альбом", ep: "EP", single: "Сингл" }[a.albumType]].filter(Boolean).join(" • ")}
              onopen={() => router.go({ name: "album", id: a.id })}
              onplay={() => player.play({ kind: "album", id: a.id })}
              menu={() => albumMenu(a)}
              current={player.track?.album?.id === a.id}
            />
          {/each}
        </div>
      </section>
    {/if}
  </div>
{/if}
