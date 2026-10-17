<script lang="ts">
  // Pause bars drawn over the whole round button, in the button's own box. An icon of
  // fixed pixel size can't sit exactly in the middle of a circle whose width has the other
  // parity (it snaps half a pixel aside); sharing the circle's box keeps both symmetric.
  // `size` is the glyph's height in px at the button's nominal size `box`.
  let { size = 20, box = 40 }: { size?: number; box?: number } = $props();
  const h = $derived((size / box) * 100 * 0.8);
  const bar = $derived(h * 0.34);
  const gap = $derived(h * 0.25);
  const x = $derived(50 - bar - gap / 2);
  const y = $derived(50 - h / 2);
</script>

<svg viewBox="0 0 100 100" class="pointer-events-none absolute inset-0 size-full" aria-hidden="true">
  <rect {x} {y} width={bar} height={h} rx={bar * 0.35} fill="currentColor" />
  <rect x={50 + gap / 2} {y} width={bar} height={h} rx={bar * 0.35} fill="currentColor" />
</svg>
