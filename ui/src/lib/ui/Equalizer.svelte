<script lang="ts">
  // Animated bars next to the playing track. CSS-only, paused when playback is paused.
  let { playing }: { playing: boolean } = $props();
</script>

<span class="inline-flex h-3.5 items-end gap-[2px] text-(--accent)" aria-hidden="true">
  {#each [0, 1, 2] as i (i)}
    <span
      class="bar w-[3px] rounded-full bg-current"
      style:animation-delay="{i * -0.35}s"
      style:animation-play-state={playing ? "running" : "paused"}
    ></span>
  {/each}
</span>

<style>
  .bar {
    height: 100%;
    transform-origin: bottom;
    animation: eq 0.9s ease-in-out infinite;
  }
  @keyframes eq {
    0%,
    100% {
      transform: scaleY(0.3);
    }
    50% {
      transform: scaleY(1);
    }
  }
</style>
