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
  import { router, type LibraryTab } from "$lib/stores/router.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { prompt, toasts } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n/ru";
  import TrackList from "$lib/ui/TrackList.svelte";
  import CardGrid from "$lib/ui/CardGrid.svelte";
  import Card from "$lib/ui/Card.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import IconLibrary from "~icons/solar/music-library-2-bold";
  import IconPlay from "~icons/solar/play-bold";
  import IconShuffle from "~icons/solar/shuffle-linear";

  let { tab }: { tab: LibraryTab } = $props();

  const tabs: { id: LibraryTab; label: string; kind: ViewKind }[] = [
    { id: "tracks", label: t.library.tracks, kind: "tracks" },
    { id: "albums", label: t.library.albums, kind: "albums" },
    { id: "artists", label: t.library.artists, kind: "artists" },
    { id: "playlists", label: t.library.playlists, kind: "playlists" },
    { id: "liked", label: t.library.liked, kind: "liked" },
  ];
  const sorts: Record<ViewKind, { id: ViewSort; label: string }[]> = {
    tracks: [
      { id: "recent", label: "Недавно добавленные" },
      { id: "title", label: "По названию" },
      { id: "artist", label: "По исполнителю" },
      { id: "album", label: "По альбому" },
    ],
    liked: [
      { id: "recent", label: "Недавно понравившиеся" },
      { id: "title", label: "По названию" },
      { id: "artist", label: "По исполнителю" },
    ],
    albums: [
      { id: "recent", label: "Недавно добавленные" },
      { id: "title", label: "По названию" },
      { id: "artist", label: "По исполнителю" },
      { id: "year", label: "По году" },
    ],
    artists: [{ id: "title", label: "По имени" }],
    playlists: [
      { id: "recent", label: "Недавние" },
      { id: "title", label: "По названию" },
    ],
  };

  const kind = $derived(tabs.find((x) => x.id === tab)!.kind);
  let sort = $state<ViewSort>("recent");
  let viewId = $state<number | null>(null);
  let rows = $state<Paged<unknown> | null>(null);
  let loaded = $state(false);

  async function load(k: ViewKind, s: ViewSort) {
    const info = await invoke<ViewInfo>("library_view", { kind: k, sort: s });
    const fetch = async (offset: number, limit: number) =>
      (await invoke<Rows>("library_rows", { viewId: info.viewId, offset, limit })).items as unknown[];
    if (rows && viewId !== null && k === kind) rows.reset(info.count, fetch);
    else rows = new Paged<unknown>(info.count, fetch);
    viewId = info.viewId;
    loaded = true;
  }

  $effect(() => {
    if (!sorts[kind].some((s) => s.id === sort)) sort = sorts[kind][0].id;
    void load(kind, sort);
  });

  $effect(() => {
    const un = on<{ kinds: ViewKind[] }>("library:changed", (e) => {
      if (e.kinds.includes(kind) || e.kinds.includes("tracks")) void load(kind, sort);
    });
    return () => void un.then((f) => f());
  });

  async function importFolder() {
    const path = await prompt.ask("Путь к папке с .webm / .m4a / .opus");
    if (!path) return;
    const n = await invoke<number>("dev_import_folder", { path });
    toasts.push(`Импортировано треков: ${n}`);
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
            class="flex h-9 items-center gap-2 rounded-full bg-ink px-4 text-[13px] font-semibold text-black transition-[background-color,transform] duration-100 hover:bg-[#dcdcdc] active:scale-[0.96]"
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
            {x.label}
          </button>
        {/each}
      </div>
      <div class="flex items-center gap-3 text-[12.5px] text-ink-3">
        {#if rows}<span class="tnum">{rows.count}</span>{/if}
        {#if sorts[kind].length > 1}
          <select
            bind:value={sort}
            class="h-8 cursor-pointer rounded-md bg-fill-1 px-2 text-[12.5px] text-ink-2 outline-none ring-1 ring-line hover:bg-fill-2"
          >
            {#each sorts[kind] as s (s.id)}
              <option value={s.id} class="bg-raised">{s.label}</option>
            {/each}
          </select>
        {/if}
      </div>
    </div>
  </div>

  {#if loaded && rows && rows.count === 0}
    <EmptyState icon={IconLibrary} title={t.library.empty} hint={t.library.emptyHint}>
      <button
        type="button"
        class="h-9 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink-2 hover:bg-fill-3 hover:text-ink"
        onclick={importFolder}
      >
        Импортировать папку с аудио
      </button>
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
                subtitle={a ? [a.artists.map((x) => x.name).join(", "), a.year].filter(Boolean).join(" • ") : undefined}
                onopen={() => a && router.go({ name: "album", id: a.id })}
                onplay={() => a && player.play({ kind: "album", id: a.id })}
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
              />
            {/snippet}
          </CardGrid>
        {:else}
          <CardGrid rows={rows as Paged<Playlist>} cardHeight={232}>
            {#snippet card(p: Playlist | undefined)}
              <Card
                coverKey={p?.coverKey}
                title={p?.title}
                subtitle={p ? `${p.trackCount} треков` : undefined}
                onopen={() => p && router.go({ name: "playlist", id: p.id })}
                onplay={() => p && player.play({ kind: "playlist", id: p.id })}
              />
            {/snippet}
          </CardGrid>
        {/if}
      </div>
    {/key}
  {/if}
</div>
