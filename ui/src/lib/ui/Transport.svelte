<script lang="ts">
  // Shuffle / prev / play-pause / next / repeat.
  import { player } from "$lib/stores/player.svelte";
  import { t } from "$lib/i18n/ru";
  import IconButton from "./IconButton.svelte";
  import IconPlay from "~icons/ph/play-fill";
  import IconPause from "~icons/ph/pause-fill";
  import IconPrev from "~icons/ph/skip-back-fill";
  import IconNext from "~icons/ph/skip-forward-fill";
  import IconShuffle from "~icons/lucide/shuffle";
  import IconRepeat from "~icons/lucide/repeat";
  import IconRepeatOne from "~icons/lucide/repeat-1";
  import IconLoader from "~icons/lucide/loader-circle";

  let { big = false, minimal = false }: { big?: boolean; minimal?: boolean } = $props();

  const busy = $derived(player.state.status === "loading" || player.state.status === "buffering");
  const repeatLabel = $derived(player.state.repeat === "one" ? t.player.repeatOne : t.player.repeat);
</script>

{#if minimal}
  <!-- Bare white glyphs, as in the full-screen player. -->
  <div class="flex items-center gap-7 text-ink">
    <button type="button" aria-label={t.player.shuffle} class="transition-opacity hover:opacity-100 {player.state.shuffle ? 'text-(--accent) opacity-100' : 'opacity-45'}" onclick={() => player.toggleShuffle()}>
      <IconShuffle class="size-[17px]" />
    </button>
    <button type="button" aria-label={t.player.prev} class="opacity-90 transition-transform hover:opacity-100 active:scale-90" onclick={() => player.prev()}>
      <IconPrev class="size-[22px]" />
    </button>
    <button type="button" aria-label={player.playing ? t.player.pause : t.player.play} class="transition-transform hover:scale-105 active:scale-90" onclick={() => player.toggle()}>
      {#if busy}<IconLoader class="size-7 animate-spin" />{:else if player.playing}<IconPause class="size-7" />{:else}<IconPlay class="size-7" />{/if}
    </button>
    <button type="button" aria-label={t.player.next} class="opacity-90 transition-transform hover:opacity-100 active:scale-90" onclick={() => player.next()}>
      <IconNext class="size-[22px]" />
    </button>
    <button type="button" aria-label={repeatLabel} class="transition-opacity hover:opacity-100 {player.state.repeat !== 'off' ? 'text-(--accent) opacity-100' : 'opacity-45'}" onclick={() => player.cycleRepeat()}>
      {#if player.state.repeat === "one"}<IconRepeatOne class="size-[17px]" />{:else}<IconRepeat class="size-[17px]" />{/if}
    </button>
  </div>
{:else}
<div class="flex items-center {big ? 'gap-5' : 'gap-2'}">
  <IconButton label={t.player.shuffle} size={big ? "md" : "sm"} active={player.state.shuffle} onclick={() => player.toggleShuffle()}>
    <IconShuffle />
  </IconButton>
  <IconButton label={t.player.prev} size={big ? "lg" : "md"} onclick={() => player.prev()}>
    <IconPrev />
  </IconButton>
  <button
    type="button"
    aria-label={player.playing ? t.player.pause : t.player.play}
    class="grid place-items-center rounded-full bg-ink text-black transition-transform duration-150 hover:scale-105 active:scale-95
      {big ? 'size-16 text-[26px]' : 'size-10 text-[18px]'}"
    onclick={() => player.toggle()}
  >
    {#if busy}
      <IconLoader class="animate-spin" />
    {:else if player.playing}
      <IconPause />
    {:else}
      <IconPlay class="translate-x-[1px]" />
    {/if}
  </button>
  <IconButton label={t.player.next} size={big ? "lg" : "md"} onclick={() => player.next()}>
    <IconNext />
  </IconButton>
  <IconButton label={repeatLabel} size={big ? "md" : "sm"} active={player.state.repeat !== "off"} onclick={() => player.cycleRepeat()}>
    {#if player.state.repeat === "one"}<IconRepeatOne />{:else}<IconRepeat />{/if}
  </IconButton>
</div>
{/if}
