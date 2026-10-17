<script lang="ts">
  import type { Track } from "$lib/ipc/bindings/Track";
  import { formatTime, player } from "$lib/stores/player.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { router } from "$lib/stores/router.svelte";
  import ArtistLinks from "./ArtistLinks.svelte";
  import { trackMenu, setLiked } from "./trackMenu";
  import Cover from "./Cover.svelte";
  import Equalizer from "./Equalizer.svelte";
  import IconPlay from "~icons/solar/play-bold";
  import IconPause from "~icons/solar/pause-bold";
  import IconHeart from "~icons/solar/heart-bold";
  import IconMore from "~icons/solar/menu-dots-bold";
  import IconCloud from "~icons/solar/cloud-linear";
  import IconAlert from "~icons/solar/danger-triangle-linear";

  interface Props {
    track: Track | undefined;
    index: number;
    /** Show the track number instead of the position (album pages). */
    number?: number | null;
    showCover?: boolean;
    showAlbum?: boolean;
    onplay: () => void;
  }

  let { track, index, number, showCover = true, showAlbum = true, onplay }: Props = $props();

  const current = $derived(!!track && player.currentId === track.id);

  function openMenu(e: MouseEvent) {
    if (!track) return;
    e.preventDefault();
    menu.show(trackMenu(track), e.clientX, e.clientY);
  }
</script>

{#if track}
  <div
    role="row"
    tabindex="-1"
    class="group grid h-[52px] cursor-pointer grid-cols-[36px_minmax(0,1.6fr)_minmax(0,1fr)_32px_56px_32px] items-center gap-3 rounded-md px-3
      transition-colors duration-100 hover:bg-fill-2 {current ? 'bg-fill-1' : ''}"
    class:no-album={!showAlbum}
    onclick={(e) => !(e.target as HTMLElement).closest("button, a") && (current ? player.toggle() : onplay())}
    onkeydown={(e) => e.key === "Enter" && (current ? player.toggle() : onplay())}
    oncontextmenu={openMenu}
  >
    <div class="grid place-items-center text-[13px] text-ink-3 tnum">
      {#if current}
        <span class="group-hover:hidden"><Equalizer playing={player.playing} /></span>
      {:else}
        <span class="group-hover:hidden">{number ?? index + 1}</span>
      {/if}
      <!-- On the current track the button pauses/resumes instead of restarting it. -->
      <button
        type="button"
        aria-label={current && player.playing ? "Пауза" : "Играть"}
        class="hidden text-ink group-hover:block"
        onclick={() => (current ? player.toggle() : onplay())}
      >
        {#if current && player.playing}<IconPause class="size-4" />{:else}<IconPlay class="size-4" />{/if}
      </button>
    </div>

    <div class="flex min-w-0 items-center gap-3">
      {#if showCover}
        <Cover coverKey={track.coverKey} size={40} rounded="sm" />
      {/if}
      <div class="min-w-0">
        <div class="flex items-center gap-1.5">
          <span class="truncate text-[14px] {current ? 'text-ink' : 'text-ink'}">{track.title}</span>
          {#if track.state === "remote" || track.state === "cached"}
            <IconCloud class="size-3.5 shrink-0 text-ink-3" />
          {:else if track.state === "broken"}
            <IconAlert class="size-3.5 shrink-0 text-danger" />
          {/if}
        </div>
        <div class="truncate text-[12.5px] text-ink-3">
          <ArtistLinks artists={track.artists} />
        </div>
      </div>
    </div>

    <div class="album min-w-0 truncate text-[13px] text-ink-3">
      {#if showAlbum && track.album}
        <button type="button" class="truncate hover:text-ink hover:underline" onclick={() => router.go({ name: "album", id: track.album!.id })}>
          {track.album.title}
        </button>
      {/if}
    </div>

    <button
      type="button"
      aria-label={track.liked ? "Убрать из понравившихся" : "Нравится"}
      class="grid place-items-center transition-opacity {track.liked ? 'text-ink' : 'text-ink-3 opacity-0 group-hover:opacity-100 hover:text-ink'}"
      onclick={() => setLiked(track!, !track!.liked)}
    >
      <IconHeart class="size-4" />
    </button>

    <div class="text-right text-[13px] text-ink-3 tnum">{formatTime(track.durationMs)}</div>

    <button
      type="button"
      aria-label="Ещё"
      class="grid place-items-center text-ink-3 opacity-0 group-hover:opacity-100 hover:text-ink"
      onclick={(e) => menu.showAt(trackMenu(track!), e.currentTarget)}
    >
      <IconMore class="size-4" />
    </button>
  </div>
{:else}
  <div class="flex h-[52px] items-center gap-3 px-3">
    <div class="w-9"></div>
    <div class="size-10 animate-pulse rounded-sm bg-fill-2"></div>
    <div class="h-3 w-1/3 animate-pulse rounded bg-fill-2"></div>
  </div>
{/if}

<style>
  .no-album {
    grid-template-columns: 36px minmax(0, 1fr) 0 32px 56px 32px;
  }
</style>
