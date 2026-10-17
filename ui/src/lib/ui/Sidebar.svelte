<script lang="ts">
  import { router, type Route } from "$lib/stores/router.svelte";
  import { t } from "$lib/i18n/ru";
  import type { Component } from "svelte";
  import IconHome from "~icons/solar/home-2-bold";
  import IconSearch from "~icons/solar/magnifier-bold";
  import IconLibrary from "~icons/solar/book-bookmark-bold";
  import IconDownloads from "~icons/solar/download-minimalistic-bold";
  import IconSettings from "~icons/solar/settings-bold";
  import Logo from "../../assets/logo.svg?raw";
  import { pressable, springs } from "$lib/motion";

  type Item = { label: string; icon: Component; route: Route; match: Route["name"][] };

  const items: Item[] = [
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
  const settings: Item = { label: t.nav.settings, icon: IconSettings, route: { name: "settings" }, match: ["settings"] };
</script>

{#snippet navItem(item: Item)}
  {@const active = item.match.includes(router.current.name)}
  <button
    type="button"
    data-tip={item.label}
    data-tip-pos="right"
    aria-label={item.label}
    aria-current={active ? "page" : undefined}
    class="grid size-10 place-items-center transition-colors
      {active ? 'cursor-default text-ink' : 'text-[#8a8a8e] hover:text-ink'}"
    {@attach pressable(0.92, springs.soft)}
    onclick={() => !active && router.go(item.route)}
  >
    <item.icon class="size-7" />
  </button>
{/snippet}

<nav class="flex w-(--sidebar-w) shrink-0 flex-col items-center rounded-xl bg-panel pt-3.5 pb-4" data-tauri-drag-region>
  <div class="mb-5 size-11 [&>svg]:size-full" aria-hidden="true">{@html Logo}</div>
  <div class="flex flex-col items-center gap-3.5">
    {#each items as item (item.label)}
      {@render navItem(item)}
    {/each}
  </div>
  <div class="flex-1 self-stretch" data-tauri-drag-region></div>
  {@render navItem(settings)}
</nav>
