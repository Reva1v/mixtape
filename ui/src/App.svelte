<script lang="ts">
  import { router } from "$lib/stores/router.svelte";
  import { t } from "$lib/i18n/ru";
  import TitleBar from "$lib/ui/TitleBar.svelte";
  import Sidebar from "$lib/ui/Sidebar.svelte";
  import ContextMenu from "$lib/ui/ContextMenu.svelte";
  import Toaster from "$lib/ui/Toaster.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import IconLibrary from "~icons/lucide/library-big";
  import IconSoon from "~icons/lucide/hammer";

  let titleBar: TitleBar;

  function onKey(e: KeyboardEvent) {
    if ((e.ctrlKey && e.key.toLowerCase() === "k") || (e.key === "/" && !isTyping(e))) {
      e.preventDefault();
      titleBar.focusSearch();
    } else if (e.altKey && e.key === "ArrowLeft") {
      router.back();
    } else if (e.altKey && e.key === "ArrowRight") {
      router.forward();
    }
  }

  function isTyping(e: KeyboardEvent) {
    const el = e.target as HTMLElement | null;
    return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
  }
</script>

<svelte:window onkeydown={onKey} onmouseup={(e) => e.button === 3 ? router.back() : e.button === 4 && router.forward()} />

<div class="relative flex h-screen flex-col">
  <Sidebar />
  <TitleBar bind:this={titleBar} />

  <div class="flex min-h-0 flex-1 pl-(--sidebar-w)">
    <main class="min-w-0 flex-1 overflow-y-auto rounded-tl-xl bg-fill-1 ring-1 ring-line">
      {#key router.current}
        {#if router.current.name === "home" || router.current.name === "library"}
          <EmptyState icon={IconLibrary} title={t.library.empty} hint={t.library.emptyHint} />
        {:else}
          <EmptyState icon={IconSoon} title={t.common.soon} />
        {/if}
      {/key}
    </main>
  </div>

  <footer class="flex h-(--player-h) shrink-0 items-center px-6 pl-[calc(var(--sidebar-w)+24px)] text-[13px] text-ink-3">
    {t.player.nothing}
  </footer>
</div>

<ContextMenu />
<Toaster />
