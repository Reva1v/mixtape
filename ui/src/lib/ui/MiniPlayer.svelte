<script lang="ts">
  // Bottom bar: now playing on the left, prev/play/next with a thin timeline in the
  // middle, and one capsule with the secondary controls on the right.
  import type { Component } from "svelte";
  import { player } from "$lib/stores/player.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n/ru";
  import { setLiked, trackMenu } from "./trackMenu";
  import Cover from "./Cover.svelte";
  import Transport from "./Transport.svelte";
  import Timeline from "./Timeline.svelte";
  import Volume from "./Volume.svelte";
  import IconHeart from "~icons/solar/heart-bold";
  import IconHeartLine from "~icons/solar/heart-linear";
  import IconShuffle from "~icons/solar/shuffle-linear";
  import IconRepeat from "~icons/solar/repeat-linear";
  import IconRepeatOne from "~icons/solar/repeat-one-linear";
  import IconQueue from "~icons/solar/playlist-minimalistic-linear";
  import IconLyrics from "~icons/solar/microphone-2-linear";
  import IconExpand from "~icons/solar/maximize-square-minimalistic-linear";

  const track = $derived(player.track);
</script>

{#snippet capsuleButton(label: string, Icon: Component, active: boolean, onclick: () => void, disabled = false)}
  <button
    type="button"
    aria-label={label}
    title={label}
    {disabled}
    class="relative grid size-8 place-items-center rounded-lg transition-colors disabled:opacity-30
      {active ? 'text-ink' : 'text-ink-3 hover:text-ink'}"
    {onclick}
  >
    <Icon class="size-[18px]" />
    {#if active}<span class="absolute bottom-0.5 size-1 rounded-full bg-ink"></span>{/if}
  </button>
{/snippet}

<footer class="grid h-(--player-h) shrink-0 grid-cols-[minmax(0,1fr)_minmax(0,1.3fr)_minmax(0,1fr)] items-center gap-6 pr-(--gap) pl-2">
  <div class="flex min-w-0 items-center gap-3">
    {#if track}
      <button
        type="button"
        class="shrink-0 transition-transform active:scale-95"
        aria-label={t.player.fullscreen}
        onclick={() => (layout.fullscreen = true)}
        oncontextmenu={(e) => {
          e.preventDefault();
          menu.show(trackMenu(track), e.clientX, e.clientY);
        }}
      >
        <Cover coverKey={track.coverKey} size={52} rounded="md" />
      </button>
      <div class="min-w-0">
        <button
          type="button"
          class="block max-w-full truncate text-[14px] font-medium text-ink hover:underline"
          onclick={() => track.album && router.go({ name: "album", id: track.album.id })}
        >
          {track.title}
        </button>
        <div class="truncate text-[12.5px] text-ink-3">
          {#each track.artists as a, i (i)}
            {#if i > 0},&nbsp;{/if}
            {#if a.id}
              <button type="button" class="hover:text-ink hover:underline" onclick={() => router.go({ name: "artist", id: a.id! })}>{a.name}</button>
            {:else}{a.name}{/if}
          {/each}
        </div>
      </div>
      <button
        type="button"
        aria-label={track.liked ? t.track.unlike : t.track.like}
        class="ml-1 grid size-8 shrink-0 place-items-center rounded-full transition-[color,transform] active:scale-90
          {track.liked ? 'text-ink' : 'text-ink-3 hover:text-ink'}"
        onclick={() => setLiked(track, !track.liked)}
      >
        {#if track.liked}<IconHeart class="size-[18px]" />{:else}<IconHeartLine class="size-[18px]" />{/if}
      </button>
    {/if}
  </div>

  <div class="flex flex-col items-center gap-1.5">
    <Transport />
    <Timeline class="w-full max-w-[480px]" />
  </div>

  <div class="flex justify-end">
    <div class="flex h-10 items-center gap-0.5 rounded-xl bg-fill-1 px-1 ring-1 ring-line">
      {@render capsuleButton(t.player.shuffle, IconShuffle, player.state.shuffle, () => player.toggleShuffle())}
      {@render capsuleButton(
        player.state.repeat === "one" ? t.player.repeatOne : t.player.repeat,
        player.state.repeat === "one" ? IconRepeatOne : IconRepeat,
        player.state.repeat !== "off",
        () => player.cycleRepeat(),
      )}
      <Volume compact />
      <div class="mx-1 h-4 w-px bg-line"></div>
      {@render capsuleButton(t.player.lyrics, IconLyrics, layout.drawer === "lyrics", () => layout.toggleDrawer("lyrics"))}
      {@render capsuleButton(t.player.queue, IconQueue, layout.drawer === "queue", () => layout.toggleDrawer("queue"))}
      {@render capsuleButton(t.player.fullscreen, IconExpand, false, () => (layout.fullscreen = true), !track)}
    </div>
  </div>
</footer>
