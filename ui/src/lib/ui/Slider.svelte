<script lang="ts">
  // Horizontal slider for seeking and volume. While dragging it shows the local value
  // and reports it through `oninput`; `onchange` fires once on release.
  interface Props {
    value: number;
    max?: number;
    label: string;
    oninput?: (v: number) => void;
    onchange?: (v: number) => void;
    class?: string;
  }

  let { value, max = 1, label, oninput, onchange, class: cls = "" }: Props = $props();

  let track: HTMLDivElement;
  let dragging = $state(false);
  let dragValue = $state(0);
  const shown = $derived(dragging ? dragValue : value);
  const pct = $derived(max > 0 ? Math.min(100, Math.max(0, (shown / max) * 100)) : 0);

  function valueAt(clientX: number) {
    const r = track.getBoundingClientRect();
    return Math.min(1, Math.max(0, (clientX - r.left) / r.width)) * max;
  }

  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    track.setPointerCapture(e.pointerId);
    dragging = true;
    dragValue = valueAt(e.clientX);
    oninput?.(dragValue);
  }
  function move(e: PointerEvent) {
    if (!dragging) return;
    dragValue = valueAt(e.clientX);
    oninput?.(dragValue);
  }
  function up(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    onchange?.(valueAt(e.clientX));
  }
  function key(e: KeyboardEvent) {
    const step = max / 20;
    if (e.key === "ArrowRight" || e.key === "ArrowUp") onchange?.(Math.min(max, value + step));
    else if (e.key === "ArrowLeft" || e.key === "ArrowDown") onchange?.(Math.max(0, value - step));
    else return;
    e.preventDefault();
    e.stopPropagation();
  }
</script>

<div
  bind:this={track}
  role="slider"
  tabindex="0"
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={max}
  aria-valuenow={Math.round(shown)}
  class="group relative flex h-4 cursor-pointer items-center {cls}"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onkeydown={key}
>
  <div class="relative h-1 w-full overflow-hidden rounded-full bg-fill-3 transition-[height] duration-150 group-hover:h-1.5">
    <div
      class="absolute inset-y-0 left-0 rounded-full bg-ink"
      style:width="{pct}%"
    ></div>
  </div>
  <div
    class="pointer-events-none absolute size-3 -translate-x-1/2 rounded-full bg-ink shadow transition-opacity duration-150
      {dragging ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'}"
    style:left="{pct}%"
  ></div>
</div>
