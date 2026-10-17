<script lang="ts">
  import { onMount } from "svelte";
  import { fly } from "svelte/transition";
  import { router } from "$lib/stores/router.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { handleHotkey } from "$lib/stores/hotkeys";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { on } from "$lib/ipc";
  import { t } from "$lib/i18n/ru";
  import TitleBar from "$lib/ui/TitleBar.svelte";
  import Sidebar from "$lib/ui/Sidebar.svelte";
  import MiniPlayer from "$lib/ui/MiniPlayer.svelte";
  import QueuePanel from "$lib/ui/QueuePanel.svelte";
  import ContextMenu from "$lib/ui/ContextMenu.svelte";
  import Toaster from "$lib/ui/Toaster.svelte";
  import Dialogs from "$lib/ui/Dialogs.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import Library from "./screens/Library.svelte";
  import Album from "./screens/Album.svelte";
  import Artist from "./screens/Artist.svelte";
  import Playlist from "./screens/Playlist.svelte";
  import Search from "./screens/Search.svelte";
  import FullscreenPlayer from "./screens/FullscreenPlayer.svelte";
  import IconSoon from "~icons/lucide/hammer";
  import IconLyrics from "~icons/lucide/mic-vocal";

  let titleBar: TitleBar;

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
    if (handleHotkey(e, () => titleBar.focusSearch())) e.preventDefault();
  }

  const route = $derived(router.current);
</script>

<svelte:window
  onkeydown={onKey}
  onmouseup={(e) => (e.button === 3 ? router.back() : e.button === 4 && router.forward())}
/>

<div class="relative flex h-screen flex-col">
  <Sidebar />
  <TitleBar bind:this={titleBar} />

  <div class="flex min-h-0 flex-1 pl-(--sidebar-w)">
    <main
      bind:this={layout.scrollEl}
      class="min-w-0 flex-1 overflow-y-auto rounded-tl-xl bg-[#121218]/70 ring-1 ring-line"
      style:scrollbar-gutter="stable"
    >
      {#if route.name === "library"}
        <Library tab={route.tab} />
      {:else if route.name === "album"}
        {#key route.id}<Album id={route.id} />{/key}
      {:else if route.name === "artist"}
        {#key route.id}<Artist id={route.id} />{/key}
      {:else if route.name === "playlist"}
        {#key route.id}<Playlist id={route.id} />{/key}
      {:else if route.name === "search"}
        <Search query={route.query} />
      {:else if route.name === "home"}
        <Library tab="tracks" />
      {:else}
        <EmptyState icon={IconSoon} title={t.common.soon} />
      {/if}
    </main>

    {#if layout.drawer}
      <aside
        class="w-(--drawer-w) shrink-0 overflow-hidden border-l border-line bg-[#121218]/70"
        transition:fly={{ x: 40, duration: 220 }}
      >
        {#if layout.drawer === "queue"}
          <QueuePanel />
        {:else}
          <EmptyState icon={IconLyrics} title="Тексты песен" hint="Появятся позже" />
        {/if}
      </aside>
    {/if}
  </div>

  <MiniPlayer />
</div>

{#if layout.fullscreen && player.track}
  <FullscreenPlayer />
{/if}

<ContextMenu />
<Dialogs />
<Toaster />
