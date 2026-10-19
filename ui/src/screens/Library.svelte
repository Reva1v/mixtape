<script lang="ts">
  import { invoke, on } from "$lib/ipc";
  import type { ViewInfo } from "$lib/ipc/bindings/ViewInfo";
  import type { Rows } from "$lib/ipc/bindings/Rows";
  import type { ViewKind } from "$lib/ipc/bindings/ViewKind";
  import type { ViewSort } from "$lib/ipc/bindings/ViewSort";
  import type { Track } from "$lib/ipc/bindings/Track";
  import type { Album } from "$lib/ipc/bindings/Album";
  import type { Artist } from "$lib/ipc/bindings/Artist";
  import type { Playlist } from "$lib/ipc/bindings/Playlist";
  import { Paged } from "$lib/stores/paged.svelte";
  import { pressable } from "$lib/motion";
  import Select from "$lib/ui/Select.svelte";
  import IconFind from "~icons/solar/magnifier-linear";
  import IconClear from "~icons/solar/close-circle-linear";
  import { router, type LibraryTab } from "$lib/stores/router.svelte";
  import { openAlbum } from "$lib/stores/actions";
  import { player } from "$lib/stores/player.svelte";
  import { prompt, toasts } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import TrackList from "$lib/ui/TrackList.svelte";
  import CardGrid from "$lib/ui/CardGrid.svelte";
  import Card from "$lib/ui/Card.svelte";
  import { albumMenu, artistMenu, playlistMenu } from "$lib/ui/collectionMenu";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import IconLibrary from "~icons/solar/music-library-2-bold";
  import IconPlay from "~icons/solar/play-bold";
  import IconShuffle from "~icons/solar/shuffle-linear";

  let { tab }: { tab: LibraryTab } = $props();

  // Tab labels are read from `t.library[id]` at render time, so they follow the language.
  const tabs: { id: LibraryTab; kind: ViewKind }[] = [
    { id: "tracks", kind: "tracks" },
    { id: "albums", kind: "albums" },
    { id: "artists", kind: "artists" },
    { id: "playlists", kind: "playlists" },
    { id: "liked", kind: "liked" },
  ];
  const sorts: Record<ViewKind, { id: ViewSort; label: string }[]> = $derived.by(() => {
    const s = t.libraryScreen.sort;
    return {
      tracks: [
        { id: "recent", label: s.recent },
        { id: "title", label: s.title },
        { id: "artist", label: s.artist },
        { id: "album", label: s.album },
      ],
      liked: [
        { id: "recent", label: s.recentLiked },
        { id: "title", label: s.title },
        { id: "artist", label: s.artist },
      ],
      albums: [
        { id: "recent", label: s.recent },
        { id: "title", label: s.title },
        { id: "artist", label: s.artist },
        { id: "year", label: s.year },
      ],
      artists: [{ id: "title", label: s.name }],
      playlists: [
        { id: "recent", label: s.recentPlaylists },
        { id: "title", label: s.title },
      ],
    };
  });

  const kind = $derived(tabs.find((x) => x.id === tab)!.kind);
  let sort = $state<ViewSort>("recent");
  let viewId = $state<number | null>(null);
  let rows = $state<Paged<unknown> | null>(null);
  let loaded = $state(false);

  // Narrows the open tab to rows matching every typed word (title, artist, album).
  let filterText = $state("");
  let filter = $state("");
  let filterEl = $state<HTMLInputElement>();
  $effect(() => {
    const f = filterText.trim();
    const id = setTimeout(() => (filter = f), 150);
    return () => clearTimeout(id);
  });

  // Every load bumps `seq`; an answer that comes back after a newer load started is dropped,
  // so a fast Tracks → Albums switch never renders track rows in the Albums grid.
  let seq = 0;
  // The kind the current `rows` were loaded for; a different kind always gets a fresh Paged.
  let rowsKind: ViewKind | null = null;

  async function load(k: ViewKind, s: ViewSort, f = filter) {
    const n = ++seq;
    const info = await invoke<ViewInfo>("library_view", { kind: k, sort: s, filter: f || null });
    if (n !== seq) return;
    const fetch = async (offset: number, limit: number) =>
      (await invoke<Rows>("library_rows", { viewId: info.viewId, offset, limit })).items as unknown[];
    if (rows && viewId !== null && rowsKind === k) rows.reset(info.count, fetch);
    else rows = new Paged<unknown>(info.count, fetch);
    rowsKind = k;
    viewId = info.viewId;
    loaded = true;
  }

  // A tab switch drops the old rows at once: the grid for the new kind never draws the old kind's rows.
  let lastKind: ViewKind | null = null;
  $effect.pre(() => {
    if (lastKind !== null && lastKind !== kind) {
      rows = null;
      rowsKind = null;
      viewId = null;
      loaded = false;
    }
    lastKind = kind;
  });

  $effect(() => {
    if (!sorts[kind].some((s) => s.id === sort)) sort = sorts[kind][0].id;
    void load(kind, sort, filter);
  });

  $effect(() => {
    const un = on<{ kinds: ViewKind[] }>("library:changed", (e) => {
      if (e.kinds.includes(kind) || e.kinds.includes("tracks")) void load(kind, sort);
    });
    return () => void un.then((f) => f());
  });

  async function importFolder() {
    const path = await prompt.ask(t.libraryScreen.importPrompt);
    if (!path) return;
    const n = await invoke<number>("dev_import_folder", { path });
    toasts.push(t.libraryScreen.imported(n));
  }

  function playAll(start = 0, shuffle = false) {
    if (viewId === null) return;
    if (shuffle !== player.state.shuffle) void player.toggleShuffle();
    void player.play({ kind: "view", viewId, liked: kind === "liked" }, start);
  }
</script>

<div class="pb-10">
  <div class="px-8 pt-10 pb-3">
    <div class="flex items-end justify-between gap-4">
      <h1 class="text-[28px] font-bold tracking-tight">{t.nav.library}</h1>
      {#if rows && rows.count > 0 && (kind === "tracks" || kind === "liked")}
        <div class="flex items-center gap-2">
          <button
            type="button"
            class="flex h-9 items-center gap-2 rounded-full bg-ink px-4 text-[13px] font-semibold text-black transition-[background-color] hover:bg-[#dcdcdc]"
            {@attach pressable(0.95)}
            onclick={() => playAll(0, false)}
          >
            <IconPlay class="size-4" />{t.player.play}
          </button>
          <button
            type="button"
            class="flex h-9 items-center gap-2 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3"
            onclick={() => playAll(0, true)}
          >
            <IconShuffle class="size-4" />{t.player.shuffle}
          </button>
        </div>
      {/if}
    </div>
    <div class="mt-4 flex items-center justify-between gap-4">
      <div class="flex gap-1.5" role="tablist">
        {#each tabs as x (x.id)}
          <button
            type="button"
            role="tab"
            aria-selected={x.id === tab}
            class="h-8 rounded-full px-3.5 text-[13px] font-medium transition-colors
              {x.id === tab ? 'bg-ink text-black' : 'bg-fill-1 text-ink-2 hover:bg-fill-2 hover:text-ink'}"
            onclick={() => router.replace({ name: "library", tab: x.id })}
          >
            {t.library[x.id]}
          </button>
        {/each}
      </div>
      <div class="flex items-center gap-3 text-[12.5px] text-ink-3">
        <label
          class="flex h-8 w-56 items-center gap-2 rounded-md bg-fill-1 px-2.5 ring-1 ring-line transition-colors focus-within:bg-fill-2 focus-within:ring-white/15"
        >
          <IconFind class="size-3.5 shrink-0" />
          <input
            bind:this={filterEl}
            bind:value={filterText}
            placeholder={t.libraryScreen.filter}
            spellcheck="false"
            class="min-w-0 flex-1 bg-transparent text-[12.5px] text-ink outline-none placeholder:text-ink-3"
            onkeydown={(e) => {
              if (e.key === "Escape" && filterText) {
                e.stopPropagation();
                filterText = "";
              }
            }}
          />
          {#if filterText}
            <button type="button" aria-label={t.libraryScreen.clear} class="text-ink-3 hover:text-ink" onclick={() => ((filterText = ""), filterEl?.focus())}>
              <IconClear class="size-3.5" />
            </button>
          {/if}
        </label>
        {#if rows}<span class="tnum">{rows.count}</span>{/if}
        {#if sorts[kind].length > 1}
          <Select bind:value={sort} options={sorts[kind]} label={t.libraryScreen.sort.label} />
        {/if}
      </div>
    </div>
  </div>

  {#if loaded && rows && rows.count === 0 && filter}
    <EmptyState icon={IconFind} title={t.libraryScreen.nothing} hint={t.libraryScreen.nothingHint(filter)}>
      <button
        type="button"
        class="h-9 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink-2 hover:bg-fill-3 hover:text-ink"
        onclick={() => router.go({ name: "search", query: filter })}
      >
        {t.libraryScreen.searchEverywhere}
      </button>
    </EmptyState>
  {:else if loaded && rows && rows.count === 0}
    <EmptyState icon={IconLibrary} title={t.library.empty} hint={t.library.emptyHint}>
      {#if import.meta.env.DEV}
      <button
        type="button"
        class="h-9 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink-2 hover:bg-fill-3 hover:text-ink"
        onclick={importFolder}
      >
        {t.libraryScreen.importFolder}
      </button>
      {/if}
    </EmptyState>
  {:else if rows}
    {#key viewId}
      <div class="pt-2">
        {#if kind === "tracks" || kind === "liked"}
          <TrackList rows={rows as Paged<Track>} onplay={(i) => playAll(i)} />
        {:else if kind === "albums"}
          <CardGrid rows={rows as Paged<Album>} cardHeight={232}>
            {#snippet card(a: Album | undefined)}
              <Card
                coverKey={a?.coverKey}
                title={a?.title}
                artists={a?.artists}
                meta={a?.year}
                menu={a && (() => albumMenu(a))}
                onopen={() => a && openAlbum(a)}
                onplay={() => a && player.play({ kind: "album", id: a.id })}
                current={!!a && player.track?.album?.id === a.id}
              />
            {/snippet}
          </CardGrid>
        {:else if kind === "artists"}
          <CardGrid rows={rows as Paged<Artist>} cardHeight={214}>
            {#snippet card(a: Artist | undefined)}
              <Card
                round
                coverKey={a?.imageKey}
                title={a?.name}
                onopen={() => a && router.go({ name: "artist", id: a.id })}
                menu={a && (() => artistMenu(a))}
              />
            {/snippet}
          </CardGrid>
        {:else}
          <CardGrid rows={rows as Paged<Playlist>} cardHeight={232}>
            {#snippet card(p: Playlist | undefined)}
              <Card
                coverKey={p?.coverKey}
                title={p?.title}
                subtitle={p ? t.count.tracks(p.trackCount) : undefined}
                onopen={() => p && router.go({ name: "playlist", id: p.id })}
                menu={p && (() => playlistMenu(p))}
                onplay={() => p && player.play({ kind: "playlist", id: p.id })}
              />
            {/snippet}
          </CardGrid>
        {/if}
      </div>
    {/key}
  {/if}
</div>
