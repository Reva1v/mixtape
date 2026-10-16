<script lang="ts">
  // Seek bar with elapsed/total time. Position is extrapolated every animation frame
  // while playing, so the bar moves smoothly between 10 Hz updates from Rust.
  import { formatTime, player } from "$lib/stores/player.svelte";
  import { t } from "$lib/i18n/ru";
  import Slider from "./Slider.svelte";

  let { class: cls = "", style = "", compact = false }: { class?: string; style?: string; compact?: boolean } = $props();

  let pos = $state(0);
  let scrub = $state<number | null>(null);

  $effect(() => {
    let raf = 0;
    const tick = () => {
      pos = player.livePosition();
      if (player.playing) raf = requestAnimationFrame(tick);
    };
    void player.state.positionMs;
    void player.state.status;
    tick();
    return () => cancelAnimationFrame(raf);
  });

  const duration = $derived(player.state.durationMs);
</script>

<div class="flex items-center gap-2.5 {cls}" {style}>
  <span class="w-10 text-right text-[11.5px] text-ink-3 tnum">{formatTime(scrub ?? pos)}</span>
  <Slider
    class="flex-1"
    label={t.player.fullscreen}
    value={pos}
    max={duration || 1}
    oninput={(v) => (scrub = v)}
    onchange={(v) => {
      scrub = null;
      if (duration) void player.seek(v);
    }}
  />
  <span class="w-10 text-[11.5px] text-ink-3 tnum {compact ? 'hidden' : ''}">{formatTime(duration)}</span>
</div>
