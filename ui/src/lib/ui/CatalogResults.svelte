<script lang="ts">
  // Search results from the YouTube Music catalog (US4): top result, tracks that stream
  // until saved, albums (owned ones marked), artists and playlists, with kind filters.
  import { invoke } from "$lib/ipc";
  import type { CatalogPage } from "$lib/ipc/bindings/CatalogPage";
  import type { CatalogKind } from "$lib/ipc/bindings/CatalogKind";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { net } from "$lib/stores/net.svelte";
  import { tracks as tracksLabel } from "$lib/i18n/ru";
  import { dragScroll } from "$lib/motion";
  import TrackList from "./TrackList.svelte";
  import Card from "./Card.svelte";
  import EmptyState from "./EmptyState.svelte";
  import { albumMenu, artistMenu, playlistMenu } from "./collectionMenu";
  import { trackMenu } from "./trackMenu";
  import { menu } from "$lib/stores/overlay.svelte";
  import Cover from "./Cover.svelte";
  import ArtistLinks from "./ArtistLinks.svelte";
  import IconCloud from "~icons/solar/cloud-linear";
  import IconNothing from "~icons/solar/magnifier-bug-linear";
  import IconOffline from "~icons/solar/wi-fi-router-minimalistic-linear";
  import IconPlay from "~icons/solar/play-bold";

  let { query }: { query: string } = $props();

  const KINDS: { id: CatalogKind; label: string }[] = [
    { id: "all", label: "Всё" },
    { id: "tracks", label: "Треки" },
    { id: "albums", label: "Альбомы" },
    { id: "artists", label: "Исполнители" },
    { id: "playlists", label: "Плейлисты" },
  ];
  let kind = $state<CatalogKind>("all");
  let page = $state.raw<CatalogPage | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let more = $state(false);
  const tracks = new Fixed<Track>([]);
  let seq = 0;

  $effect(() => {
    const q = query.trim();
    const k = kind;
    if (!q || !downloads.account.connected || !net.online) {
      page = null;
      return;
    }
    const mine = ++seq;
    loading = true;
    error = null;
    // Network search: wait for a pause in typing.
    const timer = setTimeout(async () => {
      try {
        const r = await invoke<CatalogPage>("catalog_search", { query: q, kind: k });
        if (mine !== seq) return;
        page = r;
        tracks.items = r.tracks;
      } catch (e) {
        if (mine === seq) error = e instanceof Error ? e.message : "Каталог не ответил";
      } finally {
        if (mine === seq) loading = false;
      }
    }, 350);
    return () => clearTimeout(timer);
  });

  async function loadMore() {
    if (!page?.continuation || more) return;
    more = true;
    const mine = seq;
    try {
      const r = await invoke<CatalogPage>("catalog_more", { kind, continuation: page.continuation });
      if (mine !== seq || !page) return;
      const seen = new Set(page.tracks.map((t) => t.id));
      page = {
        ...page,
        tracks: [...page.tracks, ...r.tracks.filter((t) => !seen.has(t.id))],
        albums: [...page.albums, ...r.albums],
        artists: [...page.artists, ...r.artists],
        playlists: [...page.playlists, ...r.playlists],
        ownedAlbums: [...page.ownedAlbums, ...r.ownedAlbums],
        continuation: r.continuation,
      };
      tracks.items = page.tracks;
    } catch {
      // Keep what is shown; the button stays for another try.
    } finally {
      more = false;
    }
  }

  function playTrack(i: number) {
    if (!page) return;
    void player.play({ kind: "tracks", trackIds: page.tracks.map((t) => t.id) }, i);
  }

  const owned = $derived(new Set(page?.ownedAlbums ?? []));
  const empty = $derived(
    !!page && !page.top && !page.tracks.length && !page.albums.length && !page.artists.length && !page.playlists.length,
  );
  const limit = $derived(kind === "all" ? 8 : Infinity);
</script>

<div class="flex gap-1.5 overflow-x-auto px-8 pb-5 [scrollbar-width:none]" {@attach dragScroll()}>
  {#each KINDS as k (k.id)}
    <button
      type="button"
      class="h-8 shrink-0 rounded-full px-3.5 text-[13px] font-medium transition-colors
        {kind === k.id ? 'bg-ink text-black' : 'bg-fill-1 text-ink-2 ring-1 ring-line hover:bg-fill-2 hover:text-ink'}"
      onclick={() => (kind = k.id)}
    >
      {k.label}
    </button>
  {/each}
</div>

{#if !downloads.account.connected}
  <EmptyState icon={IconCloud} title="Каталог YouTube Music" hint="Подключите аккаунт в настройках, чтобы искать за пределами библиотеки" />
{:else if !net.online}
  <EmptyState icon={IconOffline} title="Нет сети" hint="Каталог вернётся, когда появится связь. Библиотека работает" />
{:else if error}
  <EmptyState icon={IconNothing} title="Каталог не ответил" hint={error} />
{:else if empty}
  <EmptyState icon={IconNothing} title="Ничего не найдено" hint={`По запросу «${query}» в YouTube Music ничего нет`} />
{:else if page}
  <div class="space-y-9 transition-opacity duration-200 {loading ? 'opacity-50' : ''}">
    {#if page.top}
      {@const top = page.top}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">Лучший результат</h2>
        <div
          role="button"
          tabindex="0"
          class="group flex w-full max-w-[520px] cursor-pointer items-center gap-5 rounded-xl bg-fill-1 p-4 ring-1 ring-line transition-colors hover:bg-fill-2"
          onclick={() => {
            if (top.kind === "track") void player.play({ kind: "tracks", trackIds: [top.track.id] });
            else if (top.kind === "album") router.go({ name: "album", id: top.album.id });
            else if (top.kind === "artist") router.go({ name: "artist", id: top.artist.id });
            else router.go({ name: "playlist", id: top.playlist.id });
          }}
          onkeydown={(e) => e.key === "Enter" && (e.currentTarget as HTMLElement).click()}
          oncontextmenu={(e) => {
            e.preventDefault();
            const items =
              top.kind === "track"
                ? trackMenu(top.track)
                : top.kind === "album"
                  ? albumMenu(top.album)
                  : top.kind === "artist"
                    ? artistMenu(top.artist)
                    : playlistMenu(top.playlist);
            menu.show(items, e.clientX, e.clientY);
          }}
        >
          {#if top.kind === "artist"}
            <Cover coverKey={top.artist.imageKey} size={96} rounded="full" />
          {:else}
            <Cover
              coverKey={top.kind === "track" ? top.track.coverKey : top.kind === "album" ? top.album.coverKey : top.playlist.coverKey}
              size={96}
              rounded="md"
            />
          {/if}
          <div class="min-w-0 flex-1">
            <div class="truncate text-[22px] font-bold">
              {top.kind === "track" ? top.track.title : top.kind === "album" ? top.album.title : top.kind === "artist" ? top.artist.name : top.playlist.title}
            </div>
            <div class="mt-1 truncate text-[13.5px] text-ink-3">
              {#if top.kind === "track"}Трек · <ArtistLinks artists={top.track.artists} />
              {:else if top.kind === "album"}Альбом · <ArtistLinks artists={top.album.artists} />{top.album.year ? ` · ${top.album.year}` : ""}
              {:else if top.kind === "artist"}Исполнитель
              {:else}Плейлист{/if}
            </div>
          </div>
          {#if top.kind === "track"}
            <span class="grid size-11 shrink-0 place-items-center rounded-full bg-ink text-black opacity-0 transition-opacity group-hover:opacity-100">
              <IconPlay class="size-5" />
            </span>
          {/if}
        </div>
      </section>
    {/if}
    {#if page.tracks.length}
      <section>
        <h2 class="px-7 pb-2 text-[20px] font-bold">Треки</h2>
        <TrackList rows={tracks} onplay={playTrack} />
      </section>
    {/if}
    {#if page.artists.length}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">Исполнители</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-5">
          {#each page.artists.slice(0, limit) as a (a.id)}
            <Card round coverKey={a.imageKey} title={a.name} onopen={() => router.go({ name: "artist", id: a.id })} menu={() => artistMenu(a)} />
          {/each}
        </div>
      </section>
    {/if}
    {#if page.albums.length}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">Альбомы</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
          {#each page.albums.slice(0, limit) as a (a.id)}
            <Card
              coverKey={a.coverKey}
              title={a.title}
              artists={a.artists}
              meta={owned.has(a.id) ? "скачано" : (a.year ?? undefined)}
              onopen={() => router.go({ name: "album", id: a.id })}
              onplay={() => player.play({ kind: "album", id: a.id })}
              menu={() => albumMenu(a)}
              current={player.track?.album?.id === a.id}
            />
          {/each}
        </div>
      </section>
    {/if}
    {#if page.playlists.length}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">Плейлисты</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
          {#each page.playlists.slice(0, limit) as p (p.id)}
            <Card
              coverKey={p.coverKey}
              title={p.title}
              subtitle={p.trackCount ? tracksLabel(p.trackCount) : "Плейлист"}
              onopen={() => router.go({ name: "playlist", id: p.id })}
              onplay={() => player.play({ kind: "playlist", id: p.id })}
              menu={() => playlistMenu(p)}
            />
          {/each}
        </div>
      </section>
    {/if}
    {#if page.continuation}
      <div class="flex justify-center">
        <button
          type="button"
          class="h-9 rounded-full bg-fill-2 px-5 text-[13px] font-semibold ring-1 ring-line transition-colors hover:bg-fill-3 disabled:opacity-50"
          disabled={more}
          onclick={loadMore}
        >
          {more ? "Загружаю…" : "Показать ещё"}
        </button>
      </div>
    {/if}
  </div>
{:else if loading}
  <div class="space-y-3 px-8">
    {#each Array(6) as _, i (i)}
      <div class="flex items-center gap-3">
        <div class="size-10 animate-pulse rounded bg-fill-2"></div>
        <div class="h-3 w-1/3 animate-pulse rounded bg-fill-2"></div>
      </div>
    {/each}
  </div>
{/if}
