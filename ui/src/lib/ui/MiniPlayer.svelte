<script lang="ts">
  // Bottom bar, right of the sidebar and as wide as the content: now playing on the left
  // (its background fills up as the track plays), three buttons in the exact centre,
  // everything else in one capsule on the right.
  import type { Component } from "svelte";
  import { followProgress, formatTime, player } from "$lib/stores/player.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { visible } from "$lib/stores/visible.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import { pressable } from "$lib/motion";
  import { setLiked, trackMenu } from "./trackMenu";
  import Cover from "./Cover.svelte";
  import ArtistLinks from "./ArtistLinks.svelte";
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
  import IconMini from "~icons/solar/to-pip-linear";
  import { mini } from "$lib/stores/mini";

  const track = $derived(player.track);

  // Progress of the now-playing block, redrawn about once per pixel of movement while
  // playing (see `followProgress`); paused under the full-screen player, which covers
  // the bar. While the user scrubs, the fill follows the pointer instead.
  let pct = $state(0);
  let scrub = $state<{ x: number; ms: number } | null>(null);
  let barWidth = $state(0);
  const scrubbing = $derived(scrub !== null);
  $effect(() => {
    if (scrubbing) return;
    const live = visible.live && !layout.fullscreen;
    return followProgress(barWidth, (ms) => {
      const d = player.state.durationMs;
      pct = d > 0 ? Math.min(100, (ms / d) * 100) : 0;
    }, live);
  });

  // Press anywhere on the bar's empty space: playback jumps there at once; keep holding
  // and drag to scrub — the fill and a time bubble follow the pointer, the audio catches
  // up a few times a second, and the release lands exactly.
  const SCRUB_SEEK_MS = 180;
  let bar = $state<HTMLElement>();
  let lastSeek = 0;
  function scrubTo(e: PointerEvent) {
    if (!bar || !player.state.durationMs) return;
    const r = bar.getBoundingClientRect();
    const f = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    pct = f * 100;
    scrub = { x: f * r.width, ms: f * player.state.durationMs };
  }
  function seekNow() {
    if (!scrub) return;
    lastSeek = performance.now();
    void player.scrubTo(scrub.ms);
  }

  // Misclick guard: the whole bar seeks, so a press that just misses a button must not
  // jump the track. A press this close to a button counts as that button; right after a
  // button press (next, next…) the margin around that same button is wider.
  const NEAR_PX = 12;
  const REPEAT_PX = 40;
  const REPEAT_MS = 600;
  let lastButton: { el: HTMLElement; at: number } | null = null;
  let forward: { el: HTMLElement; x: number; y: number } | null = null;

  function distance(el: Element, x: number, y: number) {
    const r = el.getBoundingClientRect();
    return Math.hypot(Math.max(r.left - x, 0, x - r.right), Math.max(r.top - y, 0, y - r.bottom));
  }

  /** The button a press on empty space should go to instead of seeking, if any. */
  function nearButton(e: PointerEvent): HTMLElement | null {
    if (!bar) return null;
    const recent = lastButton && performance.now() - lastButton.at < REPEAT_MS ? lastButton.el : null;
    let best: HTMLElement | null = null;
    let bestD = Infinity;
    for (const el of bar.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")) {
      const d = distance(el, e.clientX, e.clientY);
      if (d < bestD && (d <= NEAR_PX || (el === recent && d <= REPEAT_PX))) {
        best = el;
        bestD = d;
      }
    }
    return best;
  }
</script>

{#snippet capsuleButton(label: string, Icon: Component, active: boolean, onclick: () => void, disabled = false)}
  <button
    type="button"
    aria-label={label}
    data-tip={label}
    {disabled}
    {@attach pressable(0.86)}
    class="grid size-8 place-items-center rounded-md transition-colors disabled:opacity-30
      {active ? 'text-ink' : 'text-[#8a8a8e] hover:text-ink'}"
    {onclick}
  >
    <Icon class="size-[19px]" />
  </button>
{/snippet}

<div
  data-bar
  bind:this={bar}
  bind:clientWidth={barWidth}
  role="slider"
  tabindex="-1"
  aria-label={t.miniPlayer.position}
  aria-valuemin={0}
  aria-valuemax={Math.round(player.state.durationMs / 1000)}
  aria-valuenow={Math.round(player.state.positionMs / 1000)}
  class="relative flex h-(--player-h) shrink-0 cursor-pointer items-center overflow-hidden rounded-xl bg-panel pr-2.5 select-none
    {scrub ? 'cursor-grabbing' : ''}"
  onpointerdown={(e) => {
    if (e.button !== 0) return;
    const pressed = (e.target as HTMLElement).closest<HTMLElement>("button");
    if (pressed) lastButton = { el: pressed, at: performance.now() };
    if (!track || pressed || (e.target as HTMLElement).closest("[data-no-seek], [role=slider]:not([data-bar])")) return;
    const near = nearButton(e);
    bar?.setPointerCapture(e.pointerId);
    if (near) {
      forward = { el: near, x: e.clientX, y: e.clientY };
      return;
    }
    scrubTo(e);
    seekNow();
  }}
  onpointermove={(e) => {
    // A press handed to a button turns into a scrub only when dragged away.
    if (forward && Math.hypot(e.clientX - forward.x, e.clientY - forward.y) > 8) {
      forward = null;
      scrubTo(e);
      seekNow();
    }
    if (!scrub) return;
    scrubTo(e);
    if (performance.now() - lastSeek > SCRUB_SEEK_MS) seekNow();
  }}
  onpointerup={(e) => {
    if (forward) {
      const el = forward.el;
      forward = null;
      lastButton = { el, at: performance.now() };
      el.click();
      return;
    }
    if (!scrub) return;
    scrubTo(e);
    seekNow();
    scrub = null;
  }}
  onpointercancel={() => {
    scrub = null;
    forward = null;
  }}
  oncontextmenu={(e) => {
    e.preventDefault();
    if (track) menu.show(trackMenu(track), e.clientX, e.clientY);
  }}
>
  {#if track}
    <!-- Full width, slid left by what is still to play: moving it needs no layout. -->
    <div
      class="glow-fill pointer-events-none absolute inset-0"
      style:transform="translateX({pct - 100}%)"
      style:background="color-mix(in srgb, var(--glow-fill) 70%, rgb(255 255 255 / 0.08))"
    ></div>
  {/if}
  {#if scrub}
    <!-- Where the scrub lands: a hairline and the time. -->
    <div class="pointer-events-none absolute inset-y-0 z-10 w-px bg-white/70" style:left="{scrub.x}px"></div>
    <div
      class="pointer-events-none absolute top-1.5 z-10 -translate-x-1/2 rounded-md bg-black/80 px-1.5 py-0.5 text-[11.5px] font-medium text-ink tnum"
      style:left="clamp(24px, {scrub.x}px, calc(100% - 24px))"
    >
      {formatTime(scrub.ms)}
    </div>
  {/if}
  <!-- Now playing -->
  {#if track}
    <div class="relative flex h-full max-w-[34%] min-w-[260px] items-center gap-3.5 pr-4 pl-4">
      <button
        type="button"
        {@attach pressable(0.94)}
        class="relative shrink-0"
        aria-label={t.player.fullscreen}
        onclick={() => (layout.fullscreen = true)}
      >
        <Cover coverKey={track.coverKey} size={40} rounded="sm" />
      </button>
      <div class="relative min-w-0">
        <button
          type="button"
          class="block max-w-full truncate text-[15px] leading-tight font-medium text-ink hover:underline"
          onclick={() => (layout.fullscreen = true)}
        >
          {track.title}
        </button>
        <div class="mt-0.5 truncate text-[13px] text-ink-3">
          <ArtistLinks artists={track.artists} />
        </div>
      </div>
      <button
        type="button"
        aria-label={track.liked ? t.track.unlike : t.track.like}
        {@attach pressable(0.8)}
        class="relative ml-auto grid size-8 shrink-0 place-items-center transition-colors
          {track.liked ? 'text-ink' : 'text-[#8a8a8e] hover:text-ink'}"
        onclick={() => setLiked(track, !track.liked)}
      >
        {#if track.liked}<IconHeart class="size-[19px]" />{:else}<IconHeartLine class="size-[19px]" />{/if}
      </button>
    </div>
  {/if}

  <!-- Exactly centred on the bar. -->
  <div class="pointer-events-none absolute inset-0 flex items-center justify-center">
    <div class="pointer-events-auto"><Transport /></div>
  </div>

  <div data-no-seek class="relative ml-auto flex h-10 items-center gap-1 rounded-[10px] bg-fill-1 px-1.5 ring-1 ring-line">
    {@render capsuleButton(t.player.shuffle, IconShuffle, player.state.shuffle, () => player.toggleShuffle())}
    {@render capsuleButton(
      player.state.repeat === "one" ? t.player.repeatOne : t.player.repeat,
      player.state.repeat === "one" ? IconRepeatOne : IconRepeat,
      player.state.repeat !== "off",
      () => player.cycleRepeat(),
    )}
    <Volume compact />
    {@render capsuleButton(t.player.lyrics, IconLyrics, false, () => layout.toggleLyrics(), !track)}
    {@render capsuleButton(t.player.fullscreen, IconExpand, false, () => (layout.fullscreen = true), !track)}
    {@render capsuleButton(t.tray.miniPlayer, IconMini, false, () => mini.set(true))}
    {@render capsuleButton(t.player.queue, IconQueue, layout.drawer === "queue", () => layout.toggleDrawer("queue"))}
  </div>
</div>
