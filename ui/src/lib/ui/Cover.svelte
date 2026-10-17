<script lang="ts">
  import { coverUrl } from "$lib/ipc";
  import IconMusic from "~icons/solar/music-note-2-linear";

  interface Props {
    coverKey?: string | null;
    /** Display size in CSS px; the nearest pre-cut thumbnail is requested. */
    size: number;
    rounded?: "sm" | "md" | "lg" | "full";
    class?: string;
  }

  let { coverKey, size, rounded = "md", class: cls = "" }: Props = $props();

  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  const SIZES = [64, 300, 1024] as const;
  type Thumb = (typeof SIZES)[number];

  /** The cut that covers the pixels actually drawn, with some slack: a slightly upscaled
   * 300 px picture looks the same and decodes into a ninth of the memory of 1024. */
  function bucket(css: number): Thumb {
    const px = css * dpr;
    if (px <= 72) return 64;
    if (css <= 240 || px <= 360) return 300;
    return 1024;
  }

  // On a load error, step down to the next smaller cut before giving up.
  // Both reset when the picture or the size changes.
  let thumb = $derived<Thumb>((void coverKey, bucket(size)));
  let failed = $derived((void coverKey, void size, false));
  const src = $derived(coverUrl(coverKey, thumb));

  function onerror() {
    const i = SIZES.indexOf(thumb);
    if (i > 0) thumb = SIZES[i - 1];
    else failed = true;
  }

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
      {onerror}
    />
  {:else}
    <div class="grid size-full place-items-center text-ink-3" style:font-size="{Math.max(12, size * 0.36)}px">
      <IconMusic />
    </div>
  {/if}
</div>
