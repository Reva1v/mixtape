<script lang="ts" module>
  import type { HomePage as Cached } from "$lib/ipc/bindings/HomePage";
  import type { HomeShelf as CachedShelf } from "$lib/ipc/bindings/HomeShelf";
  // The last data shown, so coming back to Home renders at full height at once and the
  // scroll position can be restored without a jump; a fresh copy replaces it quietly.
  let lastPage: Cached | null = null;
  let lastShelves: CachedShelf[] | null = null;
</script>

<script lang="ts">
  import { untrack, type Component } from "svelte";
  import type { TransitionConfig } from "svelte/transition";
  import { invoke, on } from "$lib/ipc";
  import type { HomePage } from "$lib/ipc/bindings/HomePage";
  import type { HomeShelf } from "$lib/ipc/bindings/HomeShelf";
  import type { ShelfItem } from "$lib/ipc/bindings/ShelfItem";
  import type { Album } from "$lib/ipc/bindings/Album";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { net } from "$lib/stores/net.svelte";
  import { openAlbum } from "$lib/stores/actions";
  import { menu, prompt, toasts, type MenuItem } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import { dragScroll, enter } from "$lib/motion";
  import Card from "$lib/ui/Card.svelte";
  import ArtistLinks from "$lib/ui/ArtistLinks.svelte";
  import { albumMenu } from "$lib/ui/collectionMenu";
  import { trackMenu } from "$lib/ui/trackMenu";
  import Cover from "$lib/ui/Cover.svelte";
  import Equalizer from "$lib/ui/Equalizer.svelte";
  import Skeleton from "$lib/ui/Skeleton.svelte";
  import TrackList from "$lib/ui/TrackList.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import IconHeart from "~icons/solar/heart-bold";
  import IconFire from "~icons/solar/fire-bold";
  import IconClock from "~icons/solar/clock-circle-bold";
  import IconHistory from "~icons/solar/history-bold";
  import IconSunrise from "~icons/solar/sunrise-bold";
  import IconSun from "~icons/solar/sun-2-bold";
  import IconSunset from "~icons/solar/sun-fog-bold";
  import IconMoon from "~icons/solar/moon-bold";
  import IconLibrary from "~icons/solar/music-library-2-bold";
  import IconMix from "~icons/solar/radio-minimalistic-linear";
  import IconPlaylist from "~icons/solar/playlist-linear";
  import IconAlbum from "~icons/solar/vinyl-linear";
  import IconArtist from "~icons/solar/microphone-3-linear";

  let page = $state.raw<HomePage | null>(lastPage);
  const top = new Fixed<Track>(lastPage?.top ?? []);

  async function load() {
    page = lastPage = await invoke<HomePage>("home_get");
    top.items = page.top;
  }

  $effect(() => {
    void load();
    const un = on("library:changed", () => void load());
    return () => void un.then((f) => f());
  });

  // Refresh "recently played" when the track changes (`currentId` changes only then; the
  // page has just loaded on mount).
  let seenTrack: string | null | undefined;
  $effect(() => {
    const now = player.currentId;
    const first = seenTrack === undefined;
    seenTrack = now;
    if (first) return;
    const id = setTimeout(() => void load(), 1500);
    return () => clearTimeout(id);
  });

  // YouTube Music shelves: fetched after the local part, cached by the core for 30 min.
  // `null` while the first answer is on its way (skeletons), `[]` when there is nothing.
  let shelves = $state.raw<HomeShelf[] | null>(lastShelves);
  const ytm = $derived(downloads.account.connected && net.online);
  let seq = 0;

  async function loadShelves(force = false) {
    const n = ++seq;
    const res = await invoke<HomeShelf[]>("home_shelves", { force }).catch(() => [] as HomeShelf[]);
    if (n === seq) shelves = lastShelves = res;
  }

  $effect(() => {
    if (!ytm) {
      seq++;
      shelves = [];
      return;
    }
    untrack(() => {
      if (!shelves?.length) shelves = null;
    });
    void loadShelves();
    // Track rows are re-read from the library on each call: likes and states stay current.
    const un = on("library:changed", () => void loadShelves());
    return () => void un.then((f) => f());
  });

  const hour = new Date().getHours();
  const greeting: { text: string; icon: Component } =
    hour < 5 ? { text: "Доброй ночи", icon: IconMoon }
    : hour < 12 ? { text: "Доброе утро", icon: IconSunrise }
    : hour < 18 ? { text: "Добрый день", icon: IconSun }
    : hour < 23 ? { text: "Добрый вечер", icon: IconSunset }
    : { text: "Доброй ночи", icon: IconMoon };

  const tiles = $derived(page ? (page.recent.length ? page.recent : page.added).slice(0, 5) : []);
  const empty = $derived(!!page && !page.recent.length && !page.added.length && !page.likedCount);

  async function importFolder() {
    const path = await prompt.ask("Путь к папке с .webm / .m4a / .opus");
    if (!path) return;
    const n = await invoke<number>("dev_import_folder", { path });
    toasts.push(`Импортировано треков: ${n}`);
  }

  // --- YouTube Music shelf items ---

  function itemKey(i: ShelfItem): string {
    switch (i.kind) {
      case "track":
        return `t:${i.track.id}`;
      case "mix":
        return `m:${i.playlistId}`;
      default:
        return `${i.kind}:${i.id}`;
    }
  }

  const shelfTracks = (s: HomeShelf) => s.items.flatMap((i) => (i.kind === "track" ? [i.track] : []));

  /** A track tile plays the shelf's tracks from that one; the playing one pauses/resumes. */
  function playFrom(s: HomeShelf, track: Track) {
    if (player.currentId === track.id) return void player.toggle();
    const ids = shelfTracks(s).map((x) => x.id);
    void player.play({ kind: "tracks", trackIds: ids }, Math.max(0, ids.indexOf(track.id)));
  }

  function startMix(playlistId: string) {
    return player.startMix(() => invoke("mix_start_playlist", { playlistId }));
  }

  function openPlaylist(id: string) {
    // «Понравившиеся» on YouTube Music is the library's liked list here.
    if (id === "VLLM") router.go({ name: "library", tab: "liked" });
    else router.go({ name: "playlist", id });
  }

  /** Opens the item's page (or plays it: tracks and mixes have no page). */
  function activate(s: HomeShelf, i: ShelfItem) {
    switch (i.kind) {
      case "track":
        return playFrom(s, i.track);
      case "album":
        return router.go({ name: "album", id: i.id });
      case "artist":
        return router.go({ name: "artist", id: i.id });
      case "playlist":
        return openPlaylist(i.id);
      case "mix":
        return void startMix(i.playlistId);
    }
  }

  function itemMenu(i: ShelfItem): MenuItem[] {
    switch (i.kind) {
      case "track":
        return trackMenu(i.track);
      case "album": {
        const artist = i.artists.find((a) => a.id);
        return [
          { label: t.track.openAlbum, icon: IconAlbum, action: () => router.go({ name: "album", id: i.id }) },
          {
            label: t.track.openArtist,
            icon: IconArtist,
            disabled: !artist,
            action: () => artist?.id && router.go({ name: "artist", id: artist.id }),
          },
        ];
      }
      case "artist":
        return [{ label: t.track.openArtist, icon: IconArtist, action: () => router.go({ name: "artist", id: i.id }) }];
      case "playlist":
        return [{ label: "Открыть плейлист", icon: IconPlaylist, action: () => openPlaylist(i.id) }];
      case "mix":
        return [{ label: t.track.mix, icon: IconMix, action: () => void startMix(i.playlistId) }];
    }
  }

  const coverOf = (i: ShelfItem) => (i.kind === "track" ? i.track.coverKey : i.coverKey);
  const titleOf = (i: ShelfItem) => (i.kind === "artist" ? i.name : i.kind === "track" ? i.track.title : i.title);
  const artistsOf = (i: ShelfItem) => (i.kind === "track" ? i.track.artists : i.kind === "album" ? i.artists : undefined);
  const subtitleOf = (i: ShelfItem) =>
    i.kind === "playlist" || i.kind === "mix" ? (i.subtitle ?? undefined) : i.kind === "artist" ? "Исполнитель" : undefined;

  /** Long song lists (quick picks) read better as compact rows, four to a column. */
  const asRows = (s: HomeShelf) => shelfTracks(s).length >= 12;

  function shelfIn(node: Element, i: number): TransitionConfig {
    return { ...enter(node, { y: 12 }), delay: Math.min(i, 5) * 70 };
  }
</script>

{#snippet section(title: string, Icon: Component)}
  <h2 class="mb-4 flex items-center gap-2.5 px-8 text-[20px] font-bold tracking-tight">
    <Icon class="size-5" />{title}
  </h2>
{/snippet}

{#snippet carousel(albums: Album[])}
  <div class="no-scrollbar -mt-2 flex snap-x gap-7 overflow-x-auto scroll-px-8 px-8 pt-2 pb-2" {@attach dragScroll()}>
    {#each albums as a (a.id)}
      <div class="w-44 shrink-0 snap-start">
        <Card
          coverKey={a.coverKey}
          title={a.title}
          artists={a.artists}
          onopen={() => openAlbum(a)}
          onplay={() => player.play({ kind: "album", id: a.id })}
          menu={() => albumMenu(a)}
                current={!!a && player.track?.album?.id === a.id}
        />
      </div>
    {/each}
  </div>
{/snippet}

{#snippet shelfCards(s: HomeShelf)}
  <div class="no-scrollbar -mt-2 flex snap-x gap-7 overflow-x-auto scroll-px-8 px-8 pt-2 pb-2" {@attach dragScroll()}>
    {#each s.items as i (itemKey(i))}
      <div class="w-44 shrink-0 snap-start">
        <Card
          coverKey={coverOf(i)}
          title={titleOf(i)}
          subtitle={subtitleOf(i)}
          artists={artistsOf(i)}
          meta={i.kind === "album" ? (i.year ?? undefined) : undefined}
          round={i.kind === "artist"}
          onopen={() => activate(s, i)}
          onplay={i.kind === "track" || i.kind === "mix" ? () => activate(s, i) : undefined}
          current={i.kind === "track" && player.currentId === i.track.id}
          menu={() => itemMenu(i)}
        />
      </div>
    {/each}
  </div>
{/snippet}

{#snippet shelfRows(s: HomeShelf)}
  <div
    class="no-scrollbar grid snap-x auto-cols-[minmax(260px,320px)] grid-flow-col grid-rows-4 gap-x-4 gap-y-1 overflow-x-auto scroll-px-8 px-8 pb-2"
    {@attach dragScroll()}
  >
    {#each s.items as i (itemKey(i))}
      {@const current = i.kind === "track" && player.currentId === i.track.id}
      <!-- A div, not a button: the artist names inside are buttons of their own. -->
      <div
        role="button"
        tabindex="0"
        class="group flex h-14 min-w-0 cursor-pointer snap-start items-center gap-3 rounded-md px-2 text-left transition-colors duration-100 hover:bg-fill-2
          {current ? 'bg-fill-1' : ''}"
        onclick={(e) => !(e.target as HTMLElement).closest("button") && activate(s, i)}
        onkeydown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), activate(s, i))}
        oncontextmenu={(e) => (e.preventDefault(), menu.show(itemMenu(i), e.clientX, e.clientY))}
      >
        <div class="relative">
          <Cover coverKey={coverOf(i)} size={44} rounded={i.kind === "artist" ? "full" : "sm"} />
          {#if current}
            <div class="absolute inset-0 grid place-items-center rounded-sm bg-black/50"><Equalizer playing={player.playing} /></div>
          {/if}
        </div>
        <div class="min-w-0">
          <div class="truncate text-[14px] font-medium">{titleOf(i)}</div>
          <div class="truncate text-[12.5px] text-ink-3">
            {#if artistsOf(i)?.length}<ArtistLinks artists={artistsOf(i)!} />{:else}{subtitleOf(i) ?? ""}{/if}
          </div>
        </div>
      </div>
    {/each}
  </div>
{/snippet}

<div class="pt-9 pb-12 [&>section:last-child]:mb-0">
  <header class="mb-5 flex items-center gap-3.5 px-8">
    <div class="grid size-9 place-items-center rounded-full bg-white/[0.08]"><greeting.icon class="size-5" /></div>
    <h1 class="text-[26px] font-bold tracking-tight">{greeting.text}</h1>
  </header>

  {#if empty}
    <EmptyState icon={IconLibrary} title={t.library.empty} hint={t.library.emptyHint}>
      <button type="button" class="h-9 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink-2 hover:bg-fill-3 hover:text-ink" onclick={importFolder}>
        Импортировать папку с аудио
      </button>
    </EmptyState>
  {:else if page}
    <div class="mb-12 grid grid-cols-3 gap-x-3 gap-y-2.5 px-8">
      <button
        type="button"
        class="group flex h-14 min-w-0 items-center gap-3.5 overflow-hidden rounded-[5px] bg-white/[0.055] pr-3 text-left transition-colors hover:bg-white/[0.09]"
        onclick={() => router.go({ name: "library", tab: "liked" })}
      >
        <div class="grid size-14 shrink-0 place-items-center bg-white/[0.06]"><IconHeart class="size-6" /></div>
        <div class="min-w-0">
          <div class="truncate text-[14px] font-medium">{t.library.liked}</div>
          <div class="text-[12.5px] text-ink-3">{t.count.tracks(page.likedCount)}</div>
        </div>
      </button>
      {#each tiles as a (a.id)}
        <!-- A div, not a button: the artist names inside are buttons of their own. -->
        <div
          role="button"
          tabindex="0"
          class="flex h-14 min-w-0 cursor-pointer items-center gap-3.5 overflow-hidden rounded-[5px] bg-white/[0.055] pr-3 text-left transition-colors hover:bg-white/[0.09]"
          onclick={() => openAlbum(a)}
          onkeydown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), openAlbum(a))}
          oncontextmenu={(e) => (e.preventDefault(), menu.show(albumMenu(a), e.clientX, e.clientY))}
        >
          <Cover coverKey={a.coverKey} size={56} rounded="sm" class="rounded-none" />
          <div class="min-w-0">
            <div class="truncate text-[14px] font-medium">{a.title}</div>
            <div class="truncate text-[12.5px] text-ink-3"><ArtistLinks artists={a.artists} /></div>
          </div>
        </div>
      {/each}
    </div>

    {#if page.recent.length}
      <section class="mb-10">
        {@render section("Сейчас в ротации", IconFire)}
        {@render carousel(page.recent)}
      </section>
    {/if}
    {#if page.top.length}
      <section class="mb-10">
        {@render section("Часто слушаю", IconHistory)}
        <TrackList rows={top} onplay={(i) => player.play({ kind: "tracks", trackIds: page!.top.map((x) => x.id) }, i)} />
      </section>
    {/if}
    {#if page.added.length}
      <section class="mb-10">
        {@render section("Недавно добавлено", IconClock)}
        {@render carousel(page.added)}
      </section>
    {/if}
  {/if}

  <!-- YouTube Music shelves go below the local part, so nothing on screen moves when they
       arrive; they wait for the local part to avoid being pushed down by it. -->
  {#if page}
    {#if shelves === null && ytm}
      {#each [0, 1] as n (n)}
        <section class="mb-10" aria-hidden="true">
          <Skeleton class="mx-8 mb-5 h-5 w-52" />
          <div class="flex gap-7 overflow-hidden px-8">
            {#each [0, 1, 2, 3, 4, 5, 6, 7] as k (k)}
              <div class="w-44 shrink-0"><Card /></div>
            {/each}
          </div>
        </section>
      {/each}
    {/if}
    {#each shelves ?? [] as s, n (`${n}:${s.title}`)}
      <section class="mb-10" in:shelfIn={n}>
        {#if s.title}
          <h2 class="mb-4 truncate px-8 text-[20px] font-bold tracking-tight">{s.title}</h2>
        {/if}
        {#if asRows(s)}
          {@render shelfRows(s)}
        {:else}
          {@render shelfCards(s)}
        {/if}
      </section>
    {/each}
  {/if}
</div>
