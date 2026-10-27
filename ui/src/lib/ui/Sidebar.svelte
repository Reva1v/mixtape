<script lang="ts">
  import { router, type Route } from "$lib/stores/router.svelte";
  import { t } from "$lib/i18n";
  import type { Component } from "svelte";
  import { navIcons } from "./navIcons";
  import { prefs } from "$lib/stores/prefs.svelte";
  import Logo from "../../assets/logo.svg?raw";
  import { pressable, springs } from "$lib/motion";
  import { layout } from "$lib/stores/layout.svelte";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { embedded, toIsland } from "$lib/ipc/embed";
  import IconOut from "~icons/solar/arrow-right-up-linear";

  type Item = { label: string; icon: Component; route: Route; match: Route["name"][] };

  const icons = $derived(navIcons[prefs.value.iconStyle] ?? navIcons.outline);
  const items: Item[] = $derived([
    { label: t.nav.home, icon: icons.home, route: { name: "home" }, match: ["home"] },
    { label: t.nav.search, icon: icons.search, route: { name: "search" }, match: ["search"] },
    {
      label: t.nav.library,
      icon: icons.library,
      route: { name: "library", tab: "tracks" },
      match: ["library", "album", "artist", "playlist"],
    },
    { label: t.stats.title, icon: icons.stats, route: { name: "stats" }, match: ["stats"] },
    { label: t.nav.downloads, icon: icons.downloads, route: { name: "downloads" }, match: ["downloads"] },
  ]);
  // Download progress for the badge on the «Загрузки» icon.
  const dl = $derived(downloads.progress);
  const dlPercent = $derived(dl.total > 0 ? Math.min(100, (dl.done / dl.total) * 100) : 0);

  const settings: Item = $derived({ label: t.nav.settings, icon: icons.settings, route: { name: "settings" }, match: ["settings"] });
</script>

{#snippet navItem(item: Item)}
  {@const active = item.match.includes(router.current.name)}
  {@const badge = item.route.name === "downloads" && dl.running}
  <button
    type="button"
    data-tip={badge ? `${item.label} · ${t.sidebar.progress(dl.done, dl.total)}${dl.paused ? t.sidebar.paused : ""}` : item.label}
    data-tip-pos="right"
    aria-label={item.label}
    aria-current={active ? "page" : undefined}
    class="relative grid size-10 place-items-center transition-colors
      {active ? 'cursor-default text-ink' : 'text-[#8a8a8e] hover:text-ink'}"
    {@attach pressable(0.92, springs.soft)}
    onclick={() => !active && router.go(item.route)}
  >
    <item.icon class="size-7" />
    {#if badge}
      <!-- Progress ring cut out of the icon's corner; dims while paused. -->
      <svg viewBox="0 0 14 14" class="absolute right-[2px] bottom-[2px] size-[14px] -rotate-90 rounded-full bg-panel" aria-hidden="true">
        <circle cx="7" cy="7" r="4.5" fill="none" stroke-width="2" class="stroke-white/20" />
        <circle
          cx="7"
          cy="7"
          r="4.5"
          fill="none"
          stroke-width="2"
          stroke-linecap="round"
          pathLength="100"
          stroke-dasharray="100"
          stroke-dashoffset={100 - dlPercent}
          class="transition-[stroke-dashoffset] duration-(--spring-smooth-ms) {dl.paused ? 'stroke-white/45' : 'stroke-white'}"
        />
      </svg>
    {/if}
  </button>
{/snippet}

<nav class="flex w-(--sidebar-w) shrink-0 flex-col items-center rounded-xl bg-panel pt-3.5 pb-4 {embedded ? 'rounded-b-[22px]' : ''}" data-tauri-drag-region>
  {#if embedded}
    <!-- Inside Droplet Island the logo opens the Mixtape app, which takes the playback over. -->
    <button
      type="button"
      aria-label={t.island.openApp}
      data-tip={t.island.openApp}
      data-tip-pos="right"
      class="relative mb-5 size-11 cursor-pointer rounded-[10px] shadow-[0_0_14px_1px_rgb(120_16_44/0.32)] [&>svg:first-child]:pointer-events-none [&>svg:first-child]:size-full"
      onclick={() => toIsland("standalone")}
      {@attach pressable(0.92, springs.soft)}
    >
      {@html Logo}
      <span class="absolute -right-1 -bottom-1 grid size-[18px] place-items-center rounded-full bg-panel text-ink ring-2 ring-panel">
        <IconOut class="size-3" />
      </span>
    </button>
  {:else}
  <button
    type="button"
    aria-label={t.nav.home}
    class="mb-5 size-11 rounded-[10px] shadow-[0_0_14px_1px_rgb(120_16_44/0.32)] [&>svg]:pointer-events-none [&>svg]:size-full"
    onclick={() =>
      router.current.name === "home"
        ? layout.scrollEl?.scrollTo({ top: 0, behavior: "smooth" })
        : router.go({ name: "home" })}
    {@attach pressable(0.92, springs.soft)}
  >
    {@html Logo}
  </button>
  {/if}
  <div class="flex flex-col items-center gap-3.5">
    {#each items as item (item.route.name)}
      {@render navItem(item)}
    {/each}
  </div>
  <div class="flex-1 self-stretch" data-tauri-drag-region></div>
  {@render navItem(settings)}
</nav>
