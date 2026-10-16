<script lang="ts">
  import type { Component, Snippet } from "svelte";

  interface Props {
    icon: Component;
    title: string;
    hint?: string;
    children?: Snippet;
  }

  let { icon: Icon, title, hint, children }: Props = $props();
</script>

<div class="flex h-full min-h-80 flex-col items-center justify-center gap-3 px-8 text-center">
  <div class="mb-1 grid size-16 place-items-center rounded-2xl bg-fill-1 text-ink-3 ring-1 ring-line">
    <Icon class="size-7" />
  </div>
  <p class="text-[15px] font-medium text-ink">{title}</p>
  {#if hint}
    <p class="max-w-sm text-[13px] leading-relaxed text-ink-3">{hint}</p>
  {/if}
  {#if children}
    <div class="mt-2">{@render children()}</div>
  {/if}
</div>
