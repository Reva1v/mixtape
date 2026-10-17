<script lang="ts">
  // Album / artist / playlist card: square cover, title, subtitle, hover play button.
  import { coverUrl } from "$lib/ipc";
  import IconPlay from "~icons/solar/play-bold";
  import IconPause from "~icons/solar/pause-bold";
  import { player } from "$lib/stores/player.svelte";
  import { liftable, pressable } from "$lib/motion";
  import IconMusic from "~icons/solar/music-note-2-linear";
  import ArtistLinks from "./ArtistLinks.svelte";
  import type { ArtistRef } from "$lib/ipc/bindings/ArtistRef";
  import { menu as contextMenu, type MenuItem } from "$lib/stores/overlay.svelte";

  interface Props {
    coverKey?: string | null;
    title?: string;
    subtitle?: string;
    /** Shown as links in the subtitle, after `subtitle` and before `meta`. */
    artists?: ArtistRef[];
    /** Trailing subtitle part, e.g. the year. */
    meta?: string | number;
    round?: boolean;
    onopen?: () => void;
    onplay?: () => void;
    /** This card is what is playing: the button pauses and resumes, and stays visible. */
    current?: boolean;
    oncontextmenu?: (e: MouseEvent) => void;
    /** Right-click menu items, built when the menu opens. */
    menu?: () => MenuItem[];
  }

  let { coverKey, title, subtitle, artists, meta, round = false, onopen, onplay, oncontextmenu, menu, current = false }: Props = $props();
  const showPause = $derived(current && player.playing);
  let failed = $state(false);
  const shape = $derived(round ? "rounded-full" : "rounded-md");
</script>

{#if title === undefined}
  <div>
    <div class="aspect-square w-full animate-pulse bg-fill-2 {shape}"></div>
    <div class="mt-3 h-3 w-2/3 animate-pulse rounded bg-fill-2"></div>
  </div>
{:else}
  <div
    class="group min-w-0"
    role="group"
    oncontextmenu={(e) => {
      oncontextmenu?.(e);
      if (menu && !e.defaultPrevented) {
        e.preventDefault();
        contextMenu.show(menu(), e.clientX, e.clientY);
      }
    }}
  >
    <div class="relative" {@attach liftable()}>
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
          aria-label={showPause ? "Пауза" : "Играть"}
          class="absolute right-2.5 bottom-2.5 grid size-11 translate-y-2 place-items-center rounded-full bg-ink text-black
            opacity-0 shadow-[0_6px_20px_-4px_rgb(0_0_0/0.7)] transition-[opacity,translate,background-color] duration-(--spring-smooth-ms) ease-out-soft
            group-hover:translate-y-0 group-hover:opacity-100 hover:bg-[#dcdcdc]
            {current ? 'translate-y-0 opacity-100' : ''}"
          {@attach pressable(0.9)}
          onclick={() => (current ? player.toggle() : onplay?.())}
        >
          {#if showPause}<IconPause class="size-5" />{:else}<IconPlay class="size-5" />{/if}
        </button>
      {/if}
    </div>
    <div class="mt-3 min-w-0 {round ? 'text-center' : ''}">
      <button type="button" class="block w-full truncate text-[14.5px] font-medium text-ink hover:underline {round ? 'text-center' : 'text-left'}" onclick={onopen}>
        {title}
      </button>
      {#if subtitle || artists?.length || meta}
        <div class="mt-0.5 truncate text-[13px] text-ink-3">
          {#if subtitle}{subtitle}{/if}{#if subtitle && artists?.length}&nbsp;•&nbsp;{/if}{#if artists?.length}<ArtistLinks
              {artists}
            />{/if}{#if meta && (subtitle || artists?.length)}&nbsp;•&nbsp;{/if}{#if meta}{meta}{/if}
        </div>
      {/if}
    </div>
  </div>
{/if}
