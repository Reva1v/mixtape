<script lang="ts">
  import type { Component } from "svelte";
  import { invoke, on } from "$lib/ipc";
  import type { HomePage } from "$lib/ipc/bindings/HomePage";
  import type { Album } from "$lib/ipc/bindings/Album";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { prompt, toasts } from "$lib/stores/overlay.svelte";
  import { t, tracks } from "$lib/i18n/ru";
  import Card from "$lib/ui/Card.svelte";
  import Cover from "$lib/ui/Cover.svelte";
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

  let page = $state.raw<HomePage | null>(null);
  const top = new Fixed<Track>([]);

  async function load() {
    page = await invoke<HomePage>("home_get");
    top.items = page.top;
  }

  $effect(() => {
    void load();
    const un = on("library:changed", () => void load());
    return () => void un.then((f) => f());
  });

  // Refresh "recently played" when the track changes.
  $effect(() => {
    void player.currentId;
    const id = setTimeout(() => void load(), 1500);
    return () => clearTimeout(id);
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

  function albumSub(a: Album) {
    return a.artists.map((x) => x.name).join(", ");
  }

  async function importFolder() {
    const path = await prompt.ask("Путь к папке с .webm / .m4a / .opus");
    if (!path) return;
    const n = await invoke<number>("dev_import_folder", { path });
    toasts.push(`Импортировано треков: ${n}`);
  }
</script>

{#snippet section(title: string, Icon: Component)}
  <h2 class="mb-4 flex items-center gap-2.5 px-8 text-[20px] font-bold tracking-tight">
    <Icon class="size-5" />{title}
  </h2>
{/snippet}

{#snippet carousel(albums: Album[])}
  <div class="no-scrollbar flex snap-x gap-7 overflow-x-auto scroll-px-8 px-8 pb-2">
    {#each albums as a (a.id)}
      <div class="w-44 shrink-0 snap-start">
        <Card
          coverKey={a.coverKey}
          title={a.title}
          subtitle={albumSub(a)}
          onopen={() => router.go({ name: "album", id: a.id })}
          onplay={() => player.play({ kind: "album", id: a.id })}
                current={!!a && player.track?.album?.id === a.id}
        />
      </div>
    {/each}
  </div>
{/snippet}

<div class="pt-9 pb-12">
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
          <div class="text-[12.5px] text-ink-3">{tracks(page.likedCount)}</div>
        </div>
      </button>
      {#each tiles as a (a.id)}
        <button
          type="button"
          class="flex h-14 min-w-0 items-center gap-3.5 overflow-hidden rounded-[5px] bg-white/[0.055] pr-3 text-left transition-colors hover:bg-white/[0.09]"
          onclick={() => router.go({ name: "album", id: a.id })}
        >
          <Cover coverKey={a.coverKey} size={56} rounded="sm" class="rounded-none" />
          <div class="min-w-0">
            <div class="truncate text-[14px] font-medium">{a.title}</div>
            <div class="truncate text-[12.5px] text-ink-3">{albumSub(a)}</div>
          </div>
        </button>
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
      <section>
        {@render section("Недавно добавлено", IconClock)}
        {@render carousel(page.added)}
      </section>
    {/if}
  {/if}
</div>
