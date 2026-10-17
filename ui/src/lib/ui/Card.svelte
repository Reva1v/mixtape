<script lang="ts">
  // Album / artist / playlist card: square cover, title, subtitle, hover play button.
  import { coverUrl } from "$lib/ipc";
  import IconPlay from "~icons/solar/play-bold";
  import IconMusic from "~icons/solar/music-note-2-linear";

  interface Props {
    coverKey?: string | null;
    title?: string;
    subtitle?: string;
    round?: boolean;
    onopen?: () => void;
    onplay?: () => void;
    oncontextmenu?: (e: MouseEvent) => void;
  }

  let { coverKey, title, subtitle, round = false, onopen, onplay, oncontextmenu }: Props = $props();
  let failed = $state(false);
  const shape = $derived(round ? "rounded-full" : "rounded-md");
</script>

{#if title === undefined}
  <div>
    <div class="aspect-square w-full animate-pulse bg-fill-2 {shape}"></div>
    <div class="mt-3 h-3 w-2/3 animate-pulse rounded bg-fill-2"></div>
  </div>
{:else}
  <div class="group min-w-0" role="group" {oncontextmenu}>
    <div class="relative transition-transform duration-300 ease-out-soft group-hover:-translate-y-0.5">
      <button
        type="button"
        class="block aspect-square w-full overflow-hidden bg-fill-2 shadow-[0_8px_24px_-12px_rgb(0_0_0/0.6)] {shape}"
        onclick={onopen}
      >
        {#if coverKey && !failed}
          <img
            src={coverUrl(coverKey, 300)}
            alt=""
            loading="lazy"
            decoding="async"
            draggable="false"
            class="size-full object-cover"
            onerror={() => (failed = true)}
          />
        {:else}
          <div class="grid size-full place-items-center text-ink-3"><IconMusic class="size-10" /></div>
        {/if}
      </button>
      {#if onplay}
        <button
          type="button"
          aria-label="Играть"
          class="absolute right-2.5 bottom-2.5 grid size-11 translate-y-1.5 place-items-center rounded-full bg-ink text-black
            opacity-0 shadow-[0_6px_20px_-4px_rgb(0_0_0/0.7)] transition-all duration-200 ease-out-soft
            group-hover:translate-y-0 group-hover:opacity-100 hover:scale-105 active:scale-95"
          onclick={onplay}
        >
          <IconPlay class="size-5" />
        </button>
      {/if}
    </div>
    <div class="mt-3 min-w-0 {round ? 'text-center' : ''}">
      <button type="button" class="block w-full truncate text-[14.5px] font-medium text-ink hover:underline {round ? 'text-center' : 'text-left'}" onclick={onopen}>
        {title}
      </button>
      {#if subtitle}
        <div class="mt-0.5 truncate text-[13px] text-ink-3">{subtitle}</div>
      {/if}
    </div>
  </div>
{/if}
