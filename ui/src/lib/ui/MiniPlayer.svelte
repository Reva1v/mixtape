<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n/ru";
  import { setLiked, trackMenu } from "./trackMenu";
  import Cover from "./Cover.svelte";
  import IconButton from "./IconButton.svelte";
  import Transport from "./Transport.svelte";
  import Timeline from "./Timeline.svelte";
  import Volume from "./Volume.svelte";
  import IconHeart from "~icons/ph/heart-fill";
  import IconHeartLine from "~icons/lucide/heart";
  import IconQueue from "~icons/lucide/list-music";
  import IconLyrics from "~icons/lucide/mic-vocal";
  import IconExpand from "~icons/lucide/maximize-2";

  const track = $derived(player.track);
</script>

<footer
  class="relative grid h-(--player-h) shrink-0 grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)_minmax(0,1fr)] items-center gap-6 pr-5 pl-[calc(var(--sidebar-w)+16px)]"
>
  <!-- Now playing -->
  <div class="flex min-w-0 items-center gap-3">
    {#if track}
      <button
        type="button"
        class="group relative shrink-0"
        aria-label={t.player.fullscreen}
        onclick={() => (layout.fullscreen = true)}
        oncontextmenu={(e) => {
          e.preventDefault();
          menu.show(trackMenu(track), e.clientX, e.clientY);
        }}
      >
        <Cover coverKey={track.coverKey} size={56} rounded="md" class="shadow-[0_6px_20px_-8px_rgb(0_0_0/0.8)]" />
        <span class="absolute inset-0 grid place-items-center rounded-md bg-black/45 opacity-0 transition-opacity group-hover:opacity-100">
          <IconExpand class="size-4" />
        </span>
      </button>
      <div class="min-w-0">
        <button
          type="button"
          class="block max-w-full truncate text-[14px] font-medium text-ink hover:underline"
          onclick={() => track.album && router.go({ name: "album", id: track.album.id })}
        >
          {track.title}
        </button>
        <div class="truncate text-[12.5px] text-ink-3">
          {#each track.artists as a, i (i)}
            {#if i > 0},&nbsp;{/if}
            {#if a.id}
              <button type="button" class="hover:text-ink hover:underline" onclick={() => router.go({ name: "artist", id: a.id! })}>{a.name}</button>
            {:else}{a.name}{/if}
          {/each}
        </div>
      </div>
      <IconButton label={track.liked ? t.track.unlike : t.track.like} size="sm" active={track.liked} onclick={() => setLiked(track, !track.liked)}>
        {#if track.liked}<IconHeart />{:else}<IconHeartLine />{/if}
      </IconButton>
    {:else}
      <span class="text-[13px] text-ink-3">{t.player.nothing}</span>
    {/if}
  </div>

  <!-- Controls -->
  <div class="flex flex-col items-center gap-1">
    <Transport />
    <Timeline class="w-full max-w-[560px]" />
  </div>

  <!-- Extras -->
  <div class="flex items-center justify-end gap-1">
    <IconButton label={t.player.lyrics} size="sm" active={layout.drawer === "lyrics"} onclick={() => layout.toggleDrawer("lyrics")}>
      <IconLyrics />
    </IconButton>
    <IconButton label={t.player.queue} size="sm" active={layout.drawer === "queue"} onclick={() => layout.toggleDrawer("queue")}>
      <IconQueue />
    </IconButton>
    <Volume />
    <IconButton label={t.player.fullscreen} size="sm" disabled={!track} onclick={() => (layout.fullscreen = true)}>
      <IconExpand />
    </IconButton>
  </div>
</footer>
