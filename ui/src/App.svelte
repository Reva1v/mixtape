<script lang="ts">
  import { onMount } from "svelte";
  import { router } from "$lib/stores/router.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { handleHotkey } from "$lib/stores/hotkeys";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { on } from "$lib/ipc";
  import { t } from "$lib/i18n/ru";
  import Sidebar from "$lib/ui/Sidebar.svelte";
  import MiniPlayer from "$lib/ui/MiniPlayer.svelte";
  import QueuePanel from "$lib/ui/QueuePanel.svelte";
  import ContextMenu from "$lib/ui/ContextMenu.svelte";
  import Toaster from "$lib/ui/Toaster.svelte";
  import Dialogs from "$lib/ui/Dialogs.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import Home from "./screens/Home.svelte";
  import Library from "./screens/Library.svelte";
  import Album from "./screens/Album.svelte";
  import Artist from "./screens/Artist.svelte";
  import Playlist from "./screens/Playlist.svelte";
  import Search from "./screens/Search.svelte";
  import FullscreenPlayer from "./screens/FullscreenPlayer.svelte";
  import IconSoon from "~icons/solar/widget-bold";
  import IconLyrics from "~icons/solar/microphone-2-linear";

  onMount(() => {
    player.init().catch((e) => console.error("player init failed", e));
    const un = on<{ level: "info" | "error"; text: string }>("toast", (e) => toasts.push(e.text, e.level));
    return () => void un.then((f) => f());
  });

  // Scroll to top on navigation, but keep the search screen's scroll while typing.
  let lastRoute = "";
  $effect(() => {
    const key = JSON.stringify(router.current.name === "search" ? { name: "search" } : router.current);
    if (key !== lastRoute) layout.scrollEl?.scrollTo({ top: 0 });
    lastRoute = key;
  });

  function onKey(e: KeyboardEvent) {
    if (handleHotkey(e)) e.preventDefault();
  }

  const route = $derived(router.current);

  // Panel content shown in the drawer; lags behind `layout.drawer` while closing.
  let shownDrawer = $state<"queue" | "lyrics" | null>(null);
  $effect(() => {
    if (layout.drawer) shownDrawer = layout.drawer;
  });
</script>

<svelte:window
  onkeydown={onKey}
  onmouseup={(e) => (e.button === 3 ? router.back() : e.button === 4 && router.forward())}
/>

<div class="flex h-screen" data-tauri-drag-region>
  <Sidebar />

  <div class="flex min-w-0 flex-1 flex-col" data-tauri-drag-region>
    <div class="flex min-h-0 flex-1 gap-(--gap) pt-(--gap) pr-(--gap)" data-tauri-drag-region>
      <div class="relative min-w-0 flex-1 overflow-hidden rounded-xl bg-panel">
        <!-- Artwork glow, fixed behind the scrolling content. -->
        <div
          class="pointer-events-none absolute inset-x-0 top-0 h-[300px] transition-[background] duration-1000"
          style:background="linear-gradient(180deg, color-mix(in srgb, var(--glow) 55%, transparent) 0%, transparent 100%)"
        ></div>
        <!-- Drag strip along the top edge, clear of the caption buttons. -->
        <div class="absolute top-0 right-[150px] left-0 z-10 h-7" data-tauri-drag-region></div>

        <main bind:this={layout.scrollEl} class="relative h-full overflow-y-auto">
          {#if route.name === "home"}
            <Home />
          {:else if route.name === "library"}
            <Library tab={route.tab} />
          {:else if route.name === "album"}
            {#key route.id}<Album id={route.id} />{/key}
          {:else if route.name === "artist"}
            {#key route.id}<Artist id={route.id} />{/key}
          {:else if route.name === "playlist"}
            {#key route.id}<Playlist id={route.id} />{/key}
          {:else if route.name === "search"}
            <Search query={route.query} />
          {:else}
            <EmptyState icon={IconSoon} title={t.common.soon} />
          {/if}
        </main>
      </div>

      <!-- Side panel: the column width animates, so the content area narrows smoothly;
           the panel stays mounted until the closing animation ends. -->
      <div
        class="shrink-0 overflow-hidden transition-[width] duration-300 ease-out-soft"
        style:width={layout.drawer ? "calc(var(--drawer-w) + var(--gap))" : "0px"}
        style:margin-left="calc(-1 * var(--gap))"
        ontransitionend={(e) => e.target === e.currentTarget && !layout.drawer && (shownDrawer = null)}
      >
        <aside
          class="ml-(--gap) h-full w-(--drawer-w) overflow-hidden rounded-xl bg-panel pt-10 transition-opacity duration-300
            {layout.drawer ? 'opacity-100' : 'opacity-0'}"
        >
          {#if shownDrawer === "queue"}
            <QueuePanel />
          {:else if shownDrawer === "lyrics"}
            <EmptyState icon={IconLyrics} title="Тексты песен" hint="Появятся позже" />
          {/if}
        </aside>
      </div>
    </div>

    <MiniPlayer />
  </div>
</div>

{#if layout.fullscreen && player.track}
  <FullscreenPlayer />
{/if}

<ContextMenu />
<Dialogs />
<Toaster />
