<script lang="ts">
  import { coverUrl } from "$lib/ipc";
  import IconMusic from "~icons/lucide/music";

  interface Props {
    coverKey?: string | null;
    /** Display size in CSS px; the nearest pre-cut thumbnail is requested. */
    size: number;
    rounded?: "sm" | "md" | "lg" | "full";
    class?: string;
  }

  let { coverKey, size, rounded = "md", class: cls = "" }: Props = $props();

  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  const thumb = $derived(size * dpr <= 64 ? 64 : size * dpr <= 300 ? 300 : 1024);
  const src = $derived(coverUrl(coverKey, thumb));
  let failed = $state(false);
  $effect(() => {
    void src;
    failed = false;
  });

  const radius = { sm: "rounded-sm", md: "rounded-md", lg: "rounded-lg", full: "rounded-full" };
</script>

<div
  class="relative shrink-0 overflow-hidden bg-fill-2 {radius[rounded]} {cls}"
  style:width="{size}px"
  style:height="{size}px"
>
  {#if src && !failed}
    <img
      {src}
      alt=""
      width={size}
      height={size}
      loading="lazy"
      decoding="async"
      draggable="false"
      class="size-full object-cover"
      onerror={() => (failed = true)}
    />
  {:else}
    <div class="grid size-full place-items-center text-ink-3" style:font-size="{Math.max(12, size * 0.36)}px">
      <IconMusic />
    </div>
  {/if}
</div>
