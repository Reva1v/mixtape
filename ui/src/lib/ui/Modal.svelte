<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import { blurIn, fadeSpring, rise } from "$lib/motion";
  import { visible } from "$lib/stores/visible.svelte";
  import { escapeLayer } from "$lib/stores/escape";

  interface Props {
    open: boolean;
    title?: string;
    onclose: () => void;
    width?: number;
    children: Snippet;
  }

  let { open, title, onclose, width = 440, children }: Props = $props();

  // The backdrop blur is recomputed whenever anything under it changes: progress bars,
  // equalizers and the full-screen backdrop hold still while a modal is open.
  $effect(() => {
    if (!open) return;
    untrack(() => visible.modals++);
    return () => untrack(() => visible.modals--);
  });

  // Esc closes only the topmost layer; see `escapeLayer`.
  $effect(() => {
    if (open) return escapeLayer(() => onclose());
  });
</script>

{#if open}
  <div
    class="fixed inset-0 z-50 grid place-items-center"
    role="presentation"
    onpointerdown={(e) => (e.target as HTMLElement).hasAttribute("data-backdrop") && onclose()}
  >
    <!-- Blur and dimming are separate layers; see `blurIn`. -->
    <div data-backdrop class="absolute inset-0 backdrop-blur-[8px]" transition:blurIn></div>
    <div data-backdrop class="absolute inset-0 bg-black/50" transition:fadeSpring></div>
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
      class="relative max-h-[85vh] overflow-auto rounded-xl bg-raised p-6 shadow-pop"
      style:width="{width}px"
      transition:rise
    >
      {#if title}
        <h2 class="mb-4 text-lg font-semibold">{title}</h2>
      {/if}
      {@render children()}
    </div>
  </div>
{/if}
