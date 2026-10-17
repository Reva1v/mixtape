<script lang="ts">
  // The window as a mini player (lib/stores/mini.ts): cover, title and artist, prev / play
  // / next, like, a hairline of progress along the bottom and a button back to the full
  // window. Every empty spot drags the window.
  import { followProgress, player } from "$lib/stores/player.svelte";
  import { visible } from "$lib/stores/visible.svelte";
  import { mini } from "$lib/stores/mini";
  import { t } from "$lib/i18n";
  import { pressable } from "$lib/motion";
  import { setLiked } from "./trackMenu";
  import Cover from "./Cover.svelte";
  import IconPlay from "~icons/solar/play-bold";
  import IconPause from "~icons/solar/pause-bold";
  import IconPrev from "~icons/solar/skip-previous-bold";
  import IconNext from "~icons/solar/skip-next-bold";
  import IconHeart from "~icons/solar/heart-bold";
  import IconHeartLine from "~icons/solar/heart-linear";
  import IconFull from "~icons/solar/quit-pip-linear";

  const track = $derived(player.track);

  // Redrawn about once per pixel of movement, and not at all while the window is hidden.
  let pct = $state(0);
  let width = $state(0);
  $effect(() =>
    followProgress(width, (ms) => {
      const d = player.state.durationMs;
      pct = d > 0 ? Math.min(100, (ms / d) * 100) : 0;
    }, visible.page),
  );

  const glyph = "grid place-items-center transition-colors";
</script>

<div
  class="relative flex h-screen items-center gap-3 overflow-hidden bg-panel pr-2 pl-3 select-none"
  data-tauri-drag-region="deep"
  bind:clientWidth={width}
>
  <!-- Artwork tint, as behind page headers. -->
  <div
    class="glow-fill pointer-events-none absolute inset-0"
    style:background="linear-gradient(90deg, color-mix(in srgb, var(--glow-fill) 45%, transparent) 0%, transparent 70%)"
  ></div>

  <Cover coverKey={track?.coverKey} size={88} rounded="md" class="pointer-events-none relative shrink-0" />

  <div class="relative flex h-full min-w-0 flex-1 flex-col justify-center gap-2 py-3">
    <div class="min-w-0 pr-8">
      <div class="truncate text-[14px] leading-tight font-medium text-ink">{track?.title ?? t.player.nothing}</div>
      {#if track}
        <div class="mt-0.5 truncate text-[12.5px] text-ink-3">{track.artists.map((a) => a.name).join(", ")}</div>
      {/if}
    </div>

    <div class="flex items-center gap-4">
      <button
        type="button"
        aria-label={t.player.prev}
        class="{glyph} text-ink-2 hover:text-ink"
        {@attach pressable()}
        onclick={() => player.prev()}
      >
        <IconPrev class="size-5" />
      </button>
      <button
        type="button"
        aria-label={player.playing ? t.player.pause : t.player.play}
        class="{glyph} size-9 rounded-full bg-ink text-black hover:bg-[#dcdcdc]"
        {@attach pressable()}
        onclick={() => player.toggle()}
      >
        {#if player.playing}<IconPause class="size-[18px]" />{:else}<IconPlay class="size-[18px]" />{/if}
      </button>
      <button
        type="button"
        aria-label={t.player.next}
        class="{glyph} text-ink-2 hover:text-ink"
        {@attach pressable()}
        onclick={() => player.next()}
      >
        <IconNext class="size-5" />
      </button>
      {#if track}
        <button
          type="button"
          aria-label={track.liked ? t.track.unlike : t.track.like}
          class="{glyph} ml-1 {track.liked ? 'text-ink' : 'text-[#8a8a8e] hover:text-ink'}"
          {@attach pressable(0.8)}
          onclick={() => setLiked(track, !track.liked)}
        >
          {#if track.liked}<IconHeart class="size-[18px]" />{:else}<IconHeartLine class="size-[18px]" />{/if}
        </button>
      {/if}
    </div>
  </div>

  <button
    type="button"
    aria-label={t.tray.fullWindow}
    data-tip={t.tray.fullWindow}
    data-tip-pos="bottom"
    class="{glyph} absolute top-2 right-2 size-7 rounded-md text-ink-3 hover:bg-white/[0.08] hover:text-ink"
    {@attach pressable(0.88)}
    onclick={() => mini.set(false)}
  >
    <IconFull class="size-[17px]" />
  </button>

  <!-- Progress: full width, slid left by what is still to play. -->
  <div class="pointer-events-none absolute inset-x-0 bottom-0 h-[3px] overflow-hidden bg-white/[0.08]">
    <div class="h-full bg-ink/80" style:transform="translateX({pct - 100}%)"></div>
  </div>
</div>
