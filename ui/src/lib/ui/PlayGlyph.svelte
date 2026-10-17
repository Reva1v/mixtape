<script lang="ts">
  // The Solar play triangle drawn over the whole round button, like PauseGlyph: it shares
  // the circle's box, so it stays centred and moves with the circle instead of snapping to
  // whole pixels on its own. Solar's 24-unit path already carries its optical nudge right.
  let { size = 20, box = 40 }: { size?: number; box?: number } = $props();
  const scale = $derived((size / box) * (100 / 24));
  const shift = $derived(50 - 12 * scale);
</script>

<svg viewBox="0 0 100 100" class="pointer-events-none absolute inset-0 size-full" aria-hidden="true">
  <path
    transform="translate({shift} {shift}) scale({scale})"
    fill="currentColor"
    d="M21.4086 9.35258C23.5305 10.5065 23.5305 13.4935 21.4086 14.6474L8.59662 21.6145C6.53435 22.736 4 21.2763 4 18.9671L4 5.0329C4 2.72368 6.53435 1.26402 8.59661 2.38548L21.4086 9.35258Z"
  />
</svg>
