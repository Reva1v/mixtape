<script lang="ts">
  import { onWheel } from "$lib/motion";
  // Horizontal slider for seeking and volume. While dragging it shows the local value
  // and reports it through `oninput`; `onchange` fires once on release.
  interface Props {
    value: number;
    max?: number;
    label: string;
    /** Read out by screen readers instead of the percentage (e.g. "1:23"). */
    valueText?: string;
    oninput?: (v: number) => void;
    onchange?: (v: number) => void;
    /** Mouse wheel step; defaults to 5 % of `max`. */
    wheelStep?: number;
    class?: string;
  }

  let { value, max = 1, label, valueText, oninput, onchange, wheelStep, class: cls = "" }: Props = $props();

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
  // Wheel up raises, wheel down lowers; the page under the slider does not scroll.
  function wheel(e: WheelEvent) {
    if (!onchange || e.deltaY === 0) return;
    e.preventDefault();
    e.stopPropagation();
    const step = wheelStep ?? max / 20;
    const next = Math.min(max, Math.max(0, value + (e.deltaY < 0 ? step : -step)));
    oninput?.(next);
    onchange(next);
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
  aria-valuemax={100}
  aria-valuenow={Math.round(pct)}
  aria-valuetext={valueText}
  class="group relative flex h-4 cursor-pointer items-center {cls}"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onkeydown={key}
  {@attach onWheel(wheel)}
>
  <!-- Fill and thumb move by transform: no layout when the value changes. -->
  <div class="relative h-1.5 w-full scale-y-[0.667] overflow-hidden rounded-full bg-fill-3 transition-transform duration-150 group-hover:scale-y-100">
    <div
      class="absolute inset-0 rounded-full bg-ink"
      style:transform="translateX({pct - 100}%)"
    ></div>
  </div>
  <div class="pointer-events-none absolute inset-0 flex items-center" style:transform="translateX({pct}%)">
    <div
      class="size-3 -translate-x-1/2 rounded-full bg-ink shadow transition-opacity duration-150
        {dragging ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'}"
    ></div>
  </div>
</div>
