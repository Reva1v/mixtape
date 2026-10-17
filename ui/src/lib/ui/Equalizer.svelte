<script lang="ts" module>
  // One shared clock drives every equalizer: twelve heights per 0.9 s cycle, sampled from
  // the ease-in-out 0.3 → 1 → 0.3 curve the bars used to animate, and all bars step
  // together. A CSS animation would redraw at the display rate (60–144 frames a second)
  // for three tiny bars; this wakes the page ~13 times a second, and only while a
  // visible equalizer is playing.
  import { cubicBezier } from "motion";

  const PERIOD = 900;
  const STEPS = 12;
  const STEP_MS = PERIOD / STEPS;
  /** Bars are offset by 5 steps (0.375 s), like the old -0.35 s animation delays. */
  const OFFSET = 5;
  const ease = cubicBezier(0.42, 0, 0.58, 1);
  const LEVELS = Array.from({ length: STEPS }, (_, k) => {
    const half = (k / STEPS) * 2;
    return 0.3 + 0.7 * ease(half <= 1 ? half : 2 - half);
  });

  const running = new Set<HTMLElement[]>();
  let timer = 0;

  function phase() {
    return Math.floor(performance.now() / STEP_MS);
  }

  function paint(bars: HTMLElement[], k = phase()) {
    bars.forEach((b, i) => (b.style.transform = `scaleY(${LEVELS[(k + i * OFFSET) % STEPS].toFixed(3)})`));
  }

  function tick() {
    const k = phase();
    for (const bars of running) paint(bars, k);
    // Land on the next step boundary, so every bar changes in the same frame.
    timer = window.setTimeout(tick, STEP_MS - (performance.now() % STEP_MS) + 1);
  }

  function setRunning(bars: HTMLElement[], on: boolean) {
    if (on) running.add(bars);
    else running.delete(bars);
    if (running.size && !timer) tick();
    else if (!running.size && timer) {
      clearTimeout(timer);
      timer = 0;
    }
  }
</script>

<script lang="ts">
  // Animated bars next to the playing track; frozen while paused, off screen, under a
  // modal or under the full-screen player.
  import { getContext } from "svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { visible } from "$lib/stores/visible.svelte";

  let { playing }: { playing: boolean } = $props();

  /** Inside the full-screen player itself (its queue), which is never covered by it. */
  const inOverlay = getContext<boolean>("fullscreen-player") === true;

  const bars: HTMLElement[] = [];
  const live = $derived(playing && visible.live && (inOverlay || !layout.fullscreen));

  $effect(() => {
    paint(bars);
    return () => setRunning(bars, false);
  });
  $effect(() => setRunning(bars, live));
</script>

<span class="inline-flex h-3.5 items-end gap-[2px] text-ink" aria-hidden="true">
  {#each [0, 1, 2] as i (i)}
    <span class="bar w-[3px] rounded-full bg-current" bind:this={bars[i]}></span>
  {/each}
</span>

<style>
  .bar {
    height: 100%;
    transform-origin: bottom;
  }
</style>
