<script lang="ts">
  import type { Snippet } from "svelte";
  import { fadeSpring, pop } from "$lib/motion";

  interface Props {
    open: boolean;
    title?: string;
    onclose: () => void;
    width?: number;
    children: Snippet;
  }

  let { open, title, onclose, width = 440, children }: Props = $props();

  function onKey(e: KeyboardEvent) {
    if (open && e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if open}
  <div
    class="fixed inset-0 z-50 grid place-items-center bg-black/50 backdrop-blur-sm"
    role="presentation"
    transition:fadeSpring
    onpointerdown={(e) => e.target === e.currentTarget && onclose()}
  >
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
      class="max-h-[85vh] overflow-auto rounded-xl bg-raised p-6 shadow-pop"
      style:width="{width}px"
      transition:pop={{ scale: 0.94, y: 8 }}
    >
      {#if title}
        <h2 class="mb-4 text-lg font-semibold">{title}</h2>
      {/if}
      {@render children()}
    </div>
  </div>
{/if}
