<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import { pressable } from "$lib/motion";

  interface Props extends HTMLButtonAttributes {
    label: string;
    size?: "sm" | "md" | "lg";
    active?: boolean;
    children: Snippet;
  }

  let { label, size = "md", active = false, children, class: cls = "", ...rest }: Props = $props();

  const sizes = { sm: "size-7 text-[15px]", md: "size-9 text-[18px]", lg: "size-11 text-[22px]" };
</script>

<button
  type="button"
  aria-label={label}
  data-tip={label}
  class="inline-grid shrink-0 place-items-center rounded-full transition-[background-color,color]
    hover:bg-fill-2 disabled:pointer-events-none disabled:opacity-35
    {active ? 'text-ink' : 'text-ink-2 hover:text-ink'} {sizes[size]} {cls}"
  {...rest}
  {@attach pressable()}
>
  {@render children()}
</button>
