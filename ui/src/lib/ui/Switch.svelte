<script lang="ts">
  // On/off switch; the knob travels on a spring.
  import { animate } from "motion";
  import { springs } from "$lib/motion";

  let { checked, label, onchange }: { checked: boolean; label: string; onchange: (v: boolean) => void } = $props();

  let knob = $state<HTMLSpanElement>();
  $effect(() => {
    if (knob) animate(knob, { x: checked ? 18 : 0 }, { type: "spring", ...springs.snappy });
  });
</script>

<button
  type="button"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  class="relative h-6 w-[42px] shrink-0 rounded-full transition-colors {checked ? 'bg-ink' : 'bg-fill-3'}"
  onclick={() => onchange(!checked)}
>
  <span
    bind:this={knob}
    class="absolute top-[3px] left-[3px] size-[18px] rounded-full shadow-sm transition-colors {checked ? 'bg-black' : 'bg-ink'}"
  ></span>
</button>
