<script lang="ts">
  import { router, type Route } from "$lib/stores/router.svelte";
  import { t } from "$lib/i18n/ru";
  import type { Component } from "svelte";
  import IconHome from "~icons/solar/home-2-bold";
  import IconSearch from "~icons/solar/magnifier-bold";
  import IconLibrary from "~icons/solar/library-bold";
  import IconDownloads from "~icons/solar/download-minimalistic-bold";
  import IconSettings from "~icons/solar/settings-bold";
  import Logo from "../../assets/logo.svg?raw";

  type Item = { label: string; icon: Component; route: Route; match: Route["name"][] };

  const top: Item[] = [
    { label: t.nav.home, icon: IconHome, route: { name: "home" }, match: ["home"] },
    { label: t.nav.search, icon: IconSearch, route: { name: "search" }, match: ["search"] },
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
    class="grid size-11 place-items-center rounded-lg transition-[color,transform] duration-150 active:scale-90
      {active ? 'text-ink' : 'text-ink-3 hover:text-ink-2'}"
    onclick={() => router.go(item.route)}
  >
    <item.icon class="size-[22px]" />
  </button>
{/snippet}

<nav class="flex w-(--sidebar-w) shrink-0 flex-col items-center gap-2 pt-3 pb-3" data-tauri-drag-region>
  <div class="mb-4 size-9 [&>svg]:size-full" aria-hidden="true">{@html Logo}</div>
  {#each top as item (item.label)}
    {@render navItem(item)}
  {/each}
  <div class="flex-1 self-stretch" data-tauri-drag-region></div>
  {#each bottom as item (item.label)}
    {@render navItem(item)}
  {/each}
</nav>
