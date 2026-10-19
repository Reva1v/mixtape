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
  import { t } from "$lib/i18n";
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
  import type { RecentItem } from "$lib/stores/recentSearch.svelte";
  import type { Album } from "$lib/ipc/bindings/Album";
  import type { Artist } from "$lib/ipc/bindings/Artist";
  import type { Playlist } from "$lib/ipc/bindings/Playlist";

  let { query, onpick }: { query: string; onpick?: (item: RecentItem) => void } = $props();

  // Remembered in «Недавнее» on the search page.
  const names = (a: { name: string }[]) => a.map((x) => x.name).join(", ");
  const pickTrack = (tr: Track) =>
    onpick?.({ kind: "track", id: tr.id, title: tr.title, subtitle: names(tr.artists), coverKey: tr.coverKey });
  const pickAlbum = (a: Album) => onpick?.({ kind: "album", id: a.id, title: a.title, subtitle: names(a.artists), coverKey: a.coverKey });
  const pickArtist = (a: Artist) => onpick?.({ kind: "artist", id: a.id, title: a.name, coverKey: a.imageKey });
  const pickPlaylist = (p: Playlist) => onpick?.({ kind: "playlist", id: p.id, title: p.title, coverKey: p.coverKey });

  const KINDS: { id: CatalogKind; label: string }[] = $derived([
    { id: "all", label: t.catalog.all },
    { id: "tracks", label: t.library.tracks },
    { id: "albums", label: t.library.albums },
    { id: "artists", label: t.library.artists },
    { id: "playlists", label: t.library.playlists },
  ]);
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
        if (mine === seq) error = e instanceof Error ? e.message : t.catalog.failed;
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
    if (page.tracks[i]) pickTrack(page.tracks[i]);
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
  <EmptyState icon={IconCloud} title={t.catalog.accountTitle} hint={t.catalog.accountHint} />
{:else if !net.online}
  <EmptyState icon={IconOffline} title={t.catalog.offlineTitle} hint={t.catalog.offlineHint} />
{:else if error}
  <EmptyState icon={IconNothing} title={t.catalog.failed} hint={error} />
{:else if empty}
  <EmptyState icon={IconNothing} title={t.catalog.emptyTitle} hint={t.catalog.emptyHint(query)} />
{:else if page}
  <div class="space-y-9 transition-opacity duration-200 {loading ? 'opacity-50' : ''}">
    {#if page.top}
      {@const top = page.top}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">{t.catalog.top}</h2>
        <div
          role="button"
          tabindex="0"
          class="group flex w-full max-w-[520px] cursor-pointer items-center gap-5 rounded-xl bg-fill-1 p-4 ring-1 ring-line transition-colors hover:bg-fill-2"
          onclick={() => {
            if (top.kind === "track") pickTrack(top.track);
            else if (top.kind === "album") pickAlbum(top.album);
            else if (top.kind === "artist") pickArtist(top.artist);
            else pickPlaylist(top.playlist);
            if (top.kind === "track") void player.play({ kind: "tracks", trackIds: [top.track.id] });
            else if (top.kind === "album") router.go({ name: "album", id: top.album.id });
            else if (top.kind === "artist") router.go({ name: "artist", id: top.artist.id });
            else router.go({ name: "playlist", id: top.playlist.id });
          }}
          onkeydown={(e) => e.key === "Enter" && e.target === e.currentTarget && (e.currentTarget as HTMLElement).click()}
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
              {#if top.kind === "track"}{t.catalog.kind.track} · <ArtistLinks artists={top.track.artists} />
              {:else if top.kind === "album"}{t.catalog.kind.album} · <ArtistLinks artists={top.album.artists} />{top.album.year ? ` · ${top.album.year}` : ""}
              {:else if top.kind === "artist"}{t.catalog.kind.artist}
              {:else}{t.catalog.kind.playlist}{/if}
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
        <h2 class="px-7 pb-2 text-[20px] font-bold">{t.library.tracks}</h2>
        <TrackList rows={tracks} onplay={playTrack} />
      </section>
    {/if}
    {#if page.artists.length}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">{t.library.artists}</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-5">
          {#each page.artists.slice(0, limit) as a (a.id)}
            <Card round coverKey={a.imageKey} title={a.name} onopen={() => {
                pickArtist(a);
                router.go({ name: "artist", id: a.id });
              }} menu={() => artistMenu(a)} />
          {/each}
        </div>
      </section>
    {/if}
    {#if page.albums.length}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">{t.library.albums}</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
          {#each page.albums.slice(0, limit) as a (a.id)}
            <Card
              coverKey={a.coverKey}
              title={a.title}
              artists={a.artists}
              meta={owned.has(a.id) ? t.catalog.downloaded : (a.year ?? undefined)}
              onopen={() => {
                pickAlbum(a);
                router.go({ name: "album", id: a.id });
              }}
              onplay={() => {
                pickAlbum(a);
                void player.play({ kind: "album", id: a.id });
              }}
              menu={() => albumMenu(a)}
              current={player.track?.album?.id === a.id}
            />
          {/each}
        </div>
      </section>
    {/if}
    {#if page.playlists.length}
      <section class="px-7">
        <h2 class="pb-4 text-[20px] font-bold">{t.library.playlists}</h2>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
          {#each page.playlists.slice(0, limit) as p (p.id)}
            <Card
              coverKey={p.coverKey}
              title={p.title}
              subtitle={p.trackCount ? t.count.tracks(p.trackCount) : t.catalog.kind.playlist}
              onopen={() => {
                pickPlaylist(p);
                router.go({ name: "playlist", id: p.id });
              }}
              onplay={() => {
                pickPlaylist(p);
                void player.play({ kind: "playlist", id: p.id });
              }}
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
          {more ? t.catalog.loading : t.catalog.more}
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
