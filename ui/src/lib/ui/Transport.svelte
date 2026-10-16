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

  let { big = false }: { big?: boolean } = $props();

  const busy = $derived(player.state.status === "loading" || player.state.status === "buffering");
  const repeatLabel = $derived(player.state.repeat === "one" ? t.player.repeatOne : t.player.repeat);
</script>

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
