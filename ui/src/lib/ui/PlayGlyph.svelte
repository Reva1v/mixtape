<script lang="ts">
  // Play triangle drawn over the whole round button, like PauseGlyph: it shares the
  // circle's box, so it stays centred and moves with the circle instead of snapping to
  // whole pixels on its own. Nudged right by a little for optical balance.
  let { size = 20, box = 40 }: { size?: number; box?: number } = $props();
  const h = $derived((size / box) * 100 * 0.82);
  const w = $derived(h * 0.88);
  const x = $derived(50 - w / 2 + w * 0.08);
  const y = $derived(50 - h / 2);
  const d = $derived(
    `M${x} ${y + h * 0.1}Q${x} ${y} ${x + w * 0.09} ${y + h * 0.05}L${x + w * 0.95} ${50 - h * 0.05}Q${x + w} 50 ${x + w * 0.95} ${50 + h * 0.05}L${x + w * 0.09} ${y + h * 0.95}Q${x} ${y + h} ${x} ${y + h * 0.9}Z`,
  );
</script>

<svg viewBox="0 0 100 100" class="pointer-events-none absolute inset-0 size-full" aria-hidden="true">
  <path {d} fill="currentColor" />
</svg>
