<script lang="ts">
  // Playback controls. `bar`: prev / play / next for the bottom bar. `full`: the five
  // bare glyphs of the full-screen player.
  import { player } from "$lib/stores/player.svelte";
  import { t } from "$lib/i18n";
  import { pressable } from "$lib/motion";
  import IconPlay from "~icons/solar/play-bold";
  import IconPause from "~icons/solar/pause-bold";
  import PauseGlyph from "./PauseGlyph.svelte";
  import PlayGlyph from "./PlayGlyph.svelte";
  import IconPrev from "~icons/solar/skip-previous-bold";
  import IconNext from "~icons/solar/skip-next-bold";
  import IconShuffle from "~icons/solar/shuffle-linear";
  import IconRepeat from "~icons/solar/repeat-linear";
  import IconRepeatOne from "~icons/solar/repeat-one-linear";
  import IconLoader from "~icons/lucide/loader-circle";

  let { variant = "bar" }: { variant?: "bar" | "full" } = $props();

  // Show the spinner only when loading drags on, so the icon does not flicker on
  // every track change.
  let busy = $state(false);
  $effect(() => {
    const loading = player.state.status === "loading" || player.state.status === "buffering";
    if (!loading) {
      busy = false;
      return;
    }
    const id = setTimeout(() => (busy = true), 400);
    return () => clearTimeout(id);
  });
  const repeatLabel = $derived(player.state.repeat === "one" ? t.player.repeatOne : t.player.repeat);
</script>

{#snippet dot(on: boolean)}
  <span class="block size-1 rounded-full {on ? 'bg-ink' : 'bg-transparent'}"></span>
{/snippet}

{#if variant === "full"}
  <div class="flex items-center gap-8 text-ink">
    <button {@attach pressable()}
    type="button" aria-label={t.player.shuffle} class="flex flex-col items-center gap-1 pt-2 transition-opacity hover:opacity-100 {player.state.shuffle ? 'opacity-100' : 'opacity-45'}" onclick={() => player.toggleShuffle()}>
      <IconShuffle class="size-[18px]" />{@render dot(player.state.shuffle)}
    </button>
    <button {@attach pressable()}
    type="button" aria-label={t.player.prev} class="opacity-90 transition-opacity hover:opacity-100" onclick={() => player.prev()}>
      <IconPrev class="size-6" />
    </button>
    <button {@attach pressable()}
    type="button" aria-label={player.playing ? t.player.pause : t.player.play} class="transition-opacity hover:opacity-80" onclick={() => player.toggle()}>
      {#if busy}<IconLoader class="size-8 animate-spin" />{:else if player.playing}<IconPause class="size-8" />{:else}<IconPlay class="size-8" />{/if}
    </button>
    <button {@attach pressable()}
    type="button" aria-label={t.player.next} class="opacity-90 transition-opacity hover:opacity-100" onclick={() => player.next()}>
      <IconNext class="size-6" />
    </button>
    <button {@attach pressable()}
    type="button" aria-label={repeatLabel} class="flex flex-col items-center gap-1 pt-2 transition-opacity hover:opacity-100 {player.state.repeat !== 'off' ? 'opacity-100' : 'opacity-45'}" onclick={() => player.cycleRepeat()}>
      {#if player.state.repeat === "one"}<IconRepeatOne class="size-[18px]" />{:else}<IconRepeat class="size-[18px]" />{/if}
      {@render dot(player.state.repeat !== "off")}
    </button>
  </div>
{:else}
  <div class="flex items-center gap-5">
    <button {@attach pressable()}
    type="button" aria-label={t.player.prev} class="text-ink-2 transition-colors hover:text-ink" onclick={() => player.prev()}>
      <IconPrev class="size-[22px]" />
    </button>
    <button
      {@attach pressable()}
      type="button"
      aria-label={player.playing ? t.player.pause : t.player.play}
      class="relative grid size-10 place-items-center rounded-full bg-ink text-black transition-[background-color] hover:bg-[#dcdcdc]"
      onclick={() => player.toggle()}
    >
      {#if busy}<IconLoader class="size-5 animate-spin" />{:else if player.playing}<PauseGlyph size={20} box={40} />{:else}<PlayGlyph size={20} box={40} />{/if}
    </button>
    <button {@attach pressable()}
    type="button" aria-label={t.player.next} class="text-ink-2 transition-colors hover:text-ink" onclick={() => player.next()}>
      <IconNext class="size-[22px]" />
    </button>
  </div>
{/if}
