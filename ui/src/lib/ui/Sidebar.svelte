<script lang="ts">
  import { router, type Route } from "$lib/stores/router.svelte";
  import { t } from "$lib/i18n/ru";
  import type { Component } from "svelte";
  import IconHome from "~icons/lucide/house";
  import IconLibrary from "~icons/lucide/library-big";
  import IconDownloads from "~icons/lucide/arrow-down-to-line";
  import IconSettings from "~icons/lucide/settings";
  import Logo from "../../assets/logo.svg?raw";

  type Item = { label: string; icon: Component; route: Route; match: Route["name"][] };

  const top: Item[] = [
    { label: t.nav.home, icon: IconHome, route: { name: "home" }, match: ["home"] },
    {
      label: t.nav.library,
      icon: IconLibrary,
      route: { name: "library", tab: "tracks" },
      match: ["library", "album", "artist", "playlist"],
    },
    { label: t.nav.downloads, icon: IconDownloads, route: { name: "downloads" }, match: ["downloads"] },
  ];
  const bottom: Item[] = [
    { label: t.nav.settings, icon: IconSettings, route: { name: "settings" }, match: ["settings"] },
  ];
</script>

{#snippet navItem(item: Item)}
  {@const active = item.match.includes(router.current.name)}
  <button
    type="button"
    title={item.label}
    aria-label={item.label}
    aria-current={active ? "page" : undefined}
    class="relative grid size-11 place-items-center rounded-lg transition-colors duration-150
      {active ? 'bg-fill-2 text-ink' : 'text-ink-3 hover:bg-fill-1 hover:text-ink-2'}"
    onclick={() => router.go(item.route)}
  >
    {#if active}
      <span class="absolute top-1/2 -left-2.5 h-5 w-1 -translate-y-1/2 rounded-full bg-(--accent)"></span>
    {/if}
    <item.icon class="size-5" />
  </button>
{/snippet}

<nav class="absolute inset-y-0 left-0 z-20 flex w-(--sidebar-w) flex-col items-center gap-1.5 pt-2 pb-3" data-tauri-drag-region>
  <div class="mb-3 size-8" aria-hidden="true">{@html Logo}</div>
  {#each top as item (item.label)}
    {@render navItem(item)}
  {/each}
  <div class="flex-1" data-tauri-drag-region></div>
  {#each bottom as item (item.label)}
    {@render navItem(item)}
  {/each}
</nav>
