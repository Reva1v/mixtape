<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

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
  title={label}
  class="inline-grid shrink-0 place-items-center rounded-full transition-[background-color,color,transform] duration-150 ease-out-soft
    hover:bg-fill-2 active:scale-95 disabled:pointer-events-none disabled:opacity-35
    {active ? 'text-(--accent)' : 'text-ink-2 hover:text-ink'} {sizes[size]} {cls}"
  {...rest}
>
  {@render children()}
</button>
