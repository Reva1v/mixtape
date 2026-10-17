<script lang="ts">
  // Header of album / playlist / artist pages: big cover over a wash of its colours.
  import type { Snippet } from "svelte";
  import Cover from "./Cover.svelte";
  import IconPlay from "~icons/solar/play-bold";
  import IconPause from "~icons/solar/pause-bold";
  import { player } from "$lib/stores/player.svelte";
  import IconShuffle from "~icons/solar/shuffle-linear";
  import IconMix from "~icons/solar/radio-minimalistic-linear";
  import { t } from "$lib/i18n/ru";
  import { pressable } from "$lib/motion";

  interface Props {
    kind: string;
    title: string;
    coverKey?: string | null;
    palette?: string[];
    round?: boolean;
    meta?: Snippet;
    onplay?: () => void;
    /** The collection is what is playing now: the button pauses and resumes. */
    current?: boolean;
    onshuffle?: () => void;
    onmix?: () => void;
    actions?: Snippet;
  }

  let { kind, title, coverKey, palette = [], round = false, meta, onplay, onshuffle, onmix, actions, current = false }: Props = $props();
  const showPause = $derived(current && player.playing);
  const wash = $derived(palette[0] ?? "#2a2a35");
</script>

<header class="relative overflow-hidden px-7 pt-10 pb-7">
  <div
    class="pointer-events-none absolute inset-0 opacity-60"
    style:background="radial-gradient(120% 140% at 0% 0%, {wash} 0%, transparent 60%)"
  ></div>
  <div class="pointer-events-none absolute inset-x-0 bottom-0 h-24 bg-gradient-to-b from-transparent to-panel"></div>

  <div class="relative flex items-end gap-7">
    <Cover {coverKey} size={208} rounded={round ? "full" : "lg"} class="shadow-[0_24px_60px_-20px_rgb(0_0_0/0.8)]" />
    <div class="min-w-0 pb-1">
      <div class="text-[12px] font-semibold tracking-wider text-ink-2 uppercase">{kind}</div>
      <h1 class="mt-1 line-clamp-2 text-[40px] leading-[1.1] font-extrabold tracking-tight">{title}</h1>
      {#if meta}
        <div class="mt-3 flex flex-wrap items-center gap-x-1.5 text-[13px] text-ink-2">{@render meta()}</div>
      {/if}
      <div class="mt-5 flex items-center gap-2.5">
        {#if onplay}
          <button
            type="button"
            aria-label={showPause ? t.player.pause : t.player.play}
            class="grid size-12 place-items-center rounded-full bg-ink text-black
              transition-[background-color] hover:bg-[#dcdcdc]"
            {@attach pressable(0.9)}
            onclick={() => (current ? player.toggle() : onplay?.())}
          >
            {#if showPause}<IconPause class="size-5" />{:else}<IconPlay class="size-5" />{/if}
          </button>
        {/if}
        {#if onshuffle}
          <button
            type="button"
            class="flex h-10 items-center gap-2 rounded-full bg-fill-2 px-4 text-[13px] font-medium backdrop-blur hover:bg-fill-3"
            onclick={onshuffle}
          >
            <IconShuffle class="size-4" />{t.player.shuffle}
          </button>
        {/if}
        {#if onmix}
          <button
            type="button"
            class="flex h-10 items-center gap-2 rounded-full bg-fill-2 px-4 text-[13px] font-medium backdrop-blur hover:bg-fill-3"
            onclick={onmix}
          >
            <IconMix class="size-4" />{t.track.mix}
          </button>
        {/if}
        {#if actions}{@render actions()}{/if}
      </div>
    </div>
  </div>
</header>
