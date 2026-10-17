<script lang="ts">
  // On/off switch: 40×22 track, 18 px knob with an even 2 px inset on every side.
  let { checked, label, onchange }: { checked: boolean; label: string; onchange: (v: boolean) => void } = $props();
</script>

<button
  type="button"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  class="relative h-[22px] w-[40px] shrink-0 rounded-full transition-colors duration-200
    {checked ? 'bg-ink' : 'bg-white/15 hover:bg-white/20'}"
  onclick={() => onchange(!checked)}
>
  <span
    class="absolute top-[2px] left-[2px] size-[18px] rounded-full shadow-[0_1px_3px_rgb(0_0_0/0.35)]
      transition-[translate,background-color] duration-(--spring-smooth-ms) ease-out-soft
      {checked ? 'translate-x-[18px] bg-black' : 'translate-x-0 bg-ink'}"
  ></span>
</button>
