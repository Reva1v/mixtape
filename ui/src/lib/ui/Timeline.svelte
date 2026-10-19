<script lang="ts">
  // Seek bar with elapsed/total time. The position is extrapolated between updates from
  // the core and redrawn about once per pixel of movement (see `followProgress`).
  import { followProgress, formatTime, player } from "$lib/stores/player.svelte";
  import { visible } from "$lib/stores/visible.svelte";
  import { t } from "$lib/i18n";
  import Slider from "./Slider.svelte";

  let {
    class: cls = "",
    style = "",
    compact = false,
    paused = false,
  }: { class?: string; style?: string; compact?: boolean; /** Stop redrawing (e.g. while covered). */ paused?: boolean } = $props();

  let pos = $state(0);
  let scrub = $state<number | null>(null);
  let width = $state(0);

  // At most every 250 ms, so the elapsed time ticks over on time.
  $effect(() => followProgress(width, (ms) => (pos = ms), visible.live && !paused, 250));

  const duration = $derived(player.state.durationMs);
</script>

<div class="flex items-center gap-2.5 {cls}" {style} bind:clientWidth={width}>
  <span class="w-10 text-right text-[11.5px] text-ink-3 tnum">{formatTime(scrub ?? pos)}</span>
  <Slider
    class="flex-1"
    label={t.player.position}
    valueText={formatTime(scrub ?? pos)}
    value={pos}
    max={duration || 1}
    wheelStep={5000}
    oninput={(v) => (scrub = v)}
    onchange={(v) => {
      scrub = null;
      if (duration) void player.scrubTo(v);
    }}
  />
  <span class="w-10 text-[11.5px] text-ink-3 tnum {compact ? 'hidden' : ''}">{formatTime(duration)}</span>
</div>
