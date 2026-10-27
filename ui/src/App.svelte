<script lang="ts">
  import { net } from "$lib/stores/net.svelte";
  import { updates } from "$lib/stores/updates.svelte";
  import { onMount } from "svelte";
  import { router } from "$lib/stores/router.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { handleHotkey } from "$lib/stores/hotkeys";
  import { enter, fadeSpring, pressable, springWidth } from "$lib/motion";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { on } from "$lib/ipc";
  import { embedded, toIsland } from "$lib/ipc/embed";
  import { t } from "$lib/i18n";
  import Sidebar from "$lib/ui/Sidebar.svelte";
  import MiniPlayer from "$lib/ui/MiniPlayer.svelte";
  import MiniWindow from "$lib/ui/MiniWindow.svelte";
  import { mini } from "$lib/stores/mini";
  import QueuePanel from "$lib/ui/QueuePanel.svelte";
  import ContextMenu from "$lib/ui/ContextMenu.svelte";
  import Toaster from "$lib/ui/Toaster.svelte";
  import Dialogs from "$lib/ui/Dialogs.svelte";
  import Tooltip from "$lib/ui/Tooltip.svelte";
  import Shortcuts from "$lib/ui/Shortcuts.svelte";
  import ConnectAccount from "$lib/ui/ConnectAccount.svelte";
  import ImportUrlModal from "$lib/ui/ImportUrlModal.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import Home from "./screens/Home.svelte";
  import Library from "./screens/Library.svelte";
  import Album from "./screens/Album.svelte";
  import Artist from "./screens/Artist.svelte";
  import Playlist from "./screens/Playlist.svelte";
  import Search from "./screens/Search.svelte";
  import Settings from "./screens/Settings.svelte";
  import Downloads from "./screens/Downloads.svelte";
  import Stats from "./screens/Stats.svelte";
  import FullscreenPlayer from "./screens/FullscreenPlayer.svelte";
  import IconSoon from "~icons/solar/widget-bold";
  import IconBack from "~icons/solar/alt-arrow-left-linear";
  import IconForward from "~icons/solar/alt-arrow-right-linear";
  import IconLeave from "~icons/solar/close-circle-linear";

  onMount(() => {
    player.init().catch((e) => console.error("player init failed", e));
    void downloads.init();
    void net.init();
    void updates.init();
    void mini.init();
    const un = on<{ level: "info" | "error"; text: string }>("toast", (e) => toasts.push(e.text, e.level));
    // The tray menu's «Settings».
    const unSettings = on("app:open-settings", () => router.go({ name: "settings" }));
    return () => {
      void un.then((f) => f());
      void unSettings.then((f) => f());
    };
  });

  // New pages open at the top; back/forward return to where the page was scrolled.
  // The search screen keeps its scroll while typing.
  router.scrollSource = () => layout.scrollEl?.scrollTop ?? 0;
  let lastRoute = "";
  let stopRestore: (() => void) | null = null;
  $effect(() => {
    const key = JSON.stringify(router.current.name === "search" ? { name: "search" } : router.current);
    if (key !== lastRoute) {
      stopRestore?.();
      stopRestore = restoreScroll(router.restoreScroll ?? 0);
    }
    lastRoute = key;
  });

  /** Puts the page back where it was. Cached screens have their height at once; a screen
   * still filling in gets the offset the moment it is tall enough — never a visible jump
   * from the top, and the user's own scrolling cancels it. */
  function restoreScroll(top: number): (() => void) | null {
    const el = layout.scrollEl;
    if (!el) return null;
    el.scrollTop = top;
    if (top <= 0 || el.scrollTop >= top - 1) return null;
    const content = el.firstElementChild;
    let done = false;
    const stop = () => {
      if (done) return;
      done = true;
      ro.disconnect();
      clearTimeout(timer);
      el.removeEventListener("wheel", stop);
      el.removeEventListener("pointerdown", stop);
      el.removeEventListener("keydown", stop);
    };
    const ro = new ResizeObserver(() => {
      el.scrollTop = top;
      if (el.scrollTop >= top - 1) stop();
    });
    if (content) ro.observe(content);
    const timer = setTimeout(stop, 2000);
    el.addEventListener("wheel", stop, { passive: true });
    el.addEventListener("pointerdown", stop);
    el.addEventListener("keydown", stop);
    return stop;
  }

  function onKey(e: KeyboardEvent) {
    if (handleHotkey(e)) e.preventDefault();
  }

  const route = $derived(router.current);
  // Screens animate in when they change, but not while typing a search or switching tabs.
  const screenKey = $derived(
    route.name === "search" || route.name === "library" ? route.name : JSON.stringify(route),
  );

  // Panel content shown in the drawer; lags behind `layout.drawer` while closing.
  let shownDrawer = $state<"queue" | null>(null);
  let drawerCol = $state<HTMLDivElement>();
  $effect(() => {
    const open = layout.drawer;
    if (open) shownDrawer = open;
    if (!drawerCol) return;
    const css = getComputedStyle(document.documentElement);
    const width = parseFloat(css.getPropertyValue("--drawer-w")) + parseFloat(css.getPropertyValue("--gap"));
    void springWidth(drawerCol, open ? width : 0).then(() => {
      if (!layout.drawer) shownDrawer = null;
    });
  });
</script>

<svelte:window
  onkeydown={onKey}
  onmouseup={(e) => (e.button === 3 ? router.back() : e.button === 4 && router.forward())}
/>

<!-- Every block is its own rounded panel on a black window, separated by --gap. -->
<!-- Kept (hidden) under the mini player, so pages and scroll positions survive it. -->
<!-- Inside the island: more room at the bottom and rounder bottom corners, to sit in the island's
     own 32 px corners. -->
<div class="flex h-screen gap-(--gap) p-(--gap) {embedded ? 'pb-[14px]' : ''}" style:display={layout.mini ? "none" : undefined} data-tauri-drag-region>
  <Sidebar />

  <div class="flex min-w-0 flex-1 flex-col gap-(--gap)" data-tauri-drag-region>
    <div class="flex min-h-0 flex-1 gap-(--gap)" data-tauri-drag-region>
      <div class="relative min-w-0 flex-1 overflow-hidden rounded-xl bg-panel {embedded ? 'rounded-b-[22px]' : ''}">
        <!-- Artwork glow, fixed behind the scrolling content. -->
        <div
          data-page-glow
          class="pointer-events-none absolute inset-x-0 top-0 h-[300px]"
          style:--page-glow={layout.pageGlow ?? "var(--glow)"}
          style:background="linear-gradient(180deg, color-mix(in srgb, var(--page-glow) 55%, transparent) 0%, transparent 100%)"
        ></div>
        <!-- Drag strip along the top edge, clear of the caption buttons. -->
        {#if !embedded}
          <div class="absolute top-0 right-[150px] left-0 z-10 h-7" data-tauri-drag-region></div>
        {/if}
        {#if embedded}
          <!-- Inside Droplet Island there is no window: back to the island's Home instead. -->
          <!-- Up in the top-right corner, in the caption row (user 2026-10-28). -->
          <div class="absolute top-1 right-2 z-20 flex h-8 items-center">
            <button
              type="button"
              aria-label={t.island.back}
              data-tip={t.island.back}
              data-tip-pos="bottom"
              class="grid h-8 w-8 cursor-pointer place-items-center rounded-md text-ink-2 transition-[background-color,color] hover:bg-white/[0.08] hover:text-ink"
              {@attach pressable(0.88)}
              onclick={() => toIsland("close")}
            >
              <IconLeave class="size-[18px]" />
            </button>
          </div>
        {/if}
        {#if router.canBack || router.canForward}
          <!-- History arrows sit in the caption row, just left of the window buttons
               (~136 px wide), styled like them, so they never crowd page headers. -->
          <div class="absolute {embedded ? 'top-1 right-10 h-8' : 'top-1 right-[140px] h-10'} z-20 flex items-center gap-0.5" transition:fadeSpring>
            {#each [{ id: "back", label: t.nav.back, tip: t.app.backTip, ok: router.canBack, go: () => router.back(), Icon: IconBack }, { id: "forward", label: t.nav.forward, tip: t.app.forwardTip, ok: router.canForward, go: () => router.forward(), Icon: IconForward }] as b (b.id)}
              <button
                type="button"
                aria-label={b.label}
                data-tip={b.tip}
                data-tip-pos="bottom"
                disabled={!b.ok}
                class="grid h-8 w-8 place-items-center rounded-md text-ink-2 transition-[background-color,color,opacity] hover:bg-white/[0.08] hover:text-ink
                  disabled:opacity-30 disabled:hover:bg-transparent disabled:hover:text-ink-2"
                {@attach pressable(0.88)}
                onclick={b.go}
              >
                <b.Icon class="size-[18px]" />
              </button>
            {/each}
          </div>
        {/if}

        <main bind:this={layout.scrollEl} class="relative h-full overflow-x-hidden overflow-y-auto {embedded ? 'in-island-main' : ''}">
          {#key screenKey}
          <div in:enter={router.direction === "new" ? { y: 8 } : { y: 0, x: router.direction === "back" ? -24 : 24 }}>
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
          {:else if route.name === "stats"}
            <Stats />
          {:else if route.name === "downloads"}
            <Downloads />
          {:else if route.name === "settings"}
            <Settings />
          {:else}
            <EmptyState icon={IconSoon} title={t.common.soon} />
          {/if}
          </div>
          {/key}
        </main>
      </div>

      <!-- Side panel: the column width animates, so the content area narrows smoothly;
           the panel stays mounted until the closing animation ends. -->
      <div bind:this={drawerCol} class="w-0 shrink-0 overflow-hidden" style:margin-left="calc(-1 * var(--gap))">
        <aside
          class="ml-(--gap) h-full w-(--drawer-w) overflow-hidden rounded-xl bg-panel pt-10 transition-opacity duration-(--spring-smooth-ms) {embedded ? 'rounded-b-[22px]' : ''}
            {layout.drawer ? 'opacity-100' : 'opacity-0'}"
        >
          {#if shownDrawer === "queue"}
            <QueuePanel />
          {/if}
        </aside>
      </div>
    </div>

    <!-- Inside Droplet Island the island's own player is the player. -->
    {#if !embedded}<MiniPlayer />{/if}
  </div>
</div>

{#if layout.mini}
  <MiniWindow />
{:else if layout.fullscreen && player.track}
  <FullscreenPlayer />
{/if}

<ContextMenu />
{#if !layout.mini}
  <Dialogs />
  <ConnectAccount />
  <ImportUrlModal />
  <Toaster />
  <Shortcuts />
{/if}
<Tooltip />
