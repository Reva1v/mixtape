<script lang="ts">
  // Bottom bar, right of the sidebar and as wide as the content: now playing on the left
  // (its background fills up as the track plays), three buttons in the exact centre,
  // everything else in one capsule on the right.
  import type { Component } from "svelte";
  import { formatTime, player } from "$lib/stores/player.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n/ru";
  import { setLiked, trackMenu } from "./trackMenu";
  import Cover from "./Cover.svelte";
  import Transport from "./Transport.svelte";
  import Volume from "./Volume.svelte";
  import IconHeart from "~icons/solar/heart-bold";
  import IconHeartLine from "~icons/solar/heart-linear";
  import IconShuffle from "~icons/solar/shuffle-linear";
  import IconRepeat from "~icons/solar/repeat-linear";
  import IconRepeatOne from "~icons/solar/repeat-one-linear";
  import IconQueue from "~icons/solar/panel-right-linear";
  import IconLyrics from "~icons/solar/microphone-2-linear";
  import IconExpand from "~icons/solar/maximize-square-3-linear";

  const track = $derived(player.track);

  // Progress of the now-playing block, extrapolated per frame while playing.
  let pct = $state(0);
  $effect(() => {
    let raf = 0;
    const tick = () => {
      const d = player.state.durationMs;
      pct = d > 0 ? Math.min(100, (player.livePosition() / d) * 100) : 0;
      if (player.playing) raf = requestAnimationFrame(tick);
    };
    void player.state.positionMs;
    void player.state.status;
    tick();
    return () => cancelAnimationFrame(raf);
  });

  // Click or drag across the now-playing block to seek.
  let block = $state<HTMLDivElement>();
  let seeking = false;
  function seekAt(e: PointerEvent) {
    if (!block || !player.state.durationMs) return;
    const r = block.getBoundingClientRect();
    const f = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    pct = f * 100;
    return f * player.state.durationMs;
  }
</script>

{#snippet capsuleButton(label: string, Icon: Component, active: boolean, onclick: () => void, disabled = false)}
  <button
    type="button"
    aria-label={label}
    title={label}
    {disabled}
    class="grid size-8 place-items-center rounded-md transition-colors disabled:opacity-30
      {active ? 'text-ink' : 'text-[#8a8a8e] hover:text-ink'}"
    {onclick}
  >
    <Icon class="size-[19px]" />
  </button>
{/snippet}

<footer class="relative flex h-(--player-h) shrink-0 items-center pr-(--gap)">
  <!-- Now playing -->
  {#if track}
    <div
      bind:this={block}
      role="slider"
      tabindex="-1"
      aria-label="Позиция"
      aria-valuemin={0}
      aria-valuemax={player.state.durationMs}
      aria-valuenow={player.state.positionMs}
      title="{formatTime((pct / 100) * player.state.durationMs)} / {formatTime(player.state.durationMs)}"
      class="relative flex h-[60px] max-w-[34%] min-w-[260px] items-center gap-3.5 overflow-hidden rounded-lg pr-4 pl-[18px]"
      onpointerdown={(e) => {
        if ((e.target as HTMLElement).closest("button")) return;
        seeking = true;
        block?.setPointerCapture(e.pointerId);
        seekAt(e);
      }}
      onpointermove={(e) => seeking && seekAt(e)}
      onpointerup={(e) => {
        if (!seeking) return;
        seeking = false;
        const ms = seekAt(e);
        if (ms !== undefined) void player.seek(ms);
      }}
    >
      <div
        class="pointer-events-none absolute inset-y-0 left-0"
        style:width="{pct}%"
        style:background="color-mix(in srgb, var(--glow) 70%, rgb(255 255 255 / 0.08))"
      ></div>
      <button
        type="button"
        class="relative shrink-0 transition-transform active:scale-95"
        aria-label={t.player.fullscreen}
        onclick={() => (layout.fullscreen = true)}
        oncontextmenu={(e) => {
          e.preventDefault();
          menu.show(trackMenu(track), e.clientX, e.clientY);
        }}
      >
        <Cover coverKey={track.coverKey} size={40} rounded="sm" />
      </button>
      <div class="relative min-w-0">
        <button
          type="button"
          class="block max-w-full truncate text-[15px] leading-tight font-medium text-ink hover:underline"
          onclick={() => track.album && router.go({ name: "album", id: track.album.id })}
        >
          {track.title}
        </button>
        <div class="mt-0.5 truncate text-[13px] text-ink-3">
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
        class="relative ml-auto grid size-8 shrink-0 place-items-center transition-[color,transform] active:scale-90
          {track.liked ? 'text-ink' : 'text-[#8a8a8e] hover:text-ink'}"
        onclick={() => setLiked(track, !track.liked)}
      >
        {#if track.liked}<IconHeart class="size-[19px]" />{:else}<IconHeartLine class="size-[19px]" />{/if}
      </button>
    </div>
  {/if}

  <!-- Exactly centred on the bar. -->
  <div class="absolute left-1/2 -translate-x-1/2">
    <Transport />
  </div>

  <div class="ml-auto flex h-10 items-center gap-1 rounded-[10px] bg-fill-1 px-1.5 ring-1 ring-line">
    {@render capsuleButton(t.player.shuffle, IconShuffle, player.state.shuffle, () => player.toggleShuffle())}
    {@render capsuleButton(
      player.state.repeat === "one" ? t.player.repeatOne : t.player.repeat,
      player.state.repeat === "one" ? IconRepeatOne : IconRepeat,
      player.state.repeat !== "off",
      () => player.cycleRepeat(),
    )}
    <Volume compact />
    {@render capsuleButton(t.player.lyrics, IconLyrics, layout.drawer === "lyrics", () => layout.toggleDrawer("lyrics"))}
    {@render capsuleButton(t.player.fullscreen, IconExpand, false, () => (layout.fullscreen = true), !track)}
    {@render capsuleButton(t.player.queue, IconQueue, layout.drawer === "queue", () => layout.toggleDrawer("queue"))}
  </div>
</footer>
