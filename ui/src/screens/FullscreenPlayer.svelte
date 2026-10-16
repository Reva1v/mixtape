<script lang="ts">
  // Full-screen player. The backdrop is the 64 px cover stretched over the window in a
  // few slowly drifting layers: upscaling blurs it for free and only transform/opacity
  // animate, so the compositor does the work.
  import { coverUrl } from "$lib/ipc";
  import { player } from "$lib/stores/player.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { t } from "$lib/i18n/ru";
  import { fade } from "svelte/transition";
  import Cover from "$lib/ui/Cover.svelte";
  import Transport from "$lib/ui/Transport.svelte";
  import Timeline from "$lib/ui/Timeline.svelte";
  import Volume from "$lib/ui/Volume.svelte";
  import IconButton from "$lib/ui/IconButton.svelte";
  import IconCollapse from "~icons/lucide/chevron-down";
  import IconQueue from "~icons/lucide/list-music";
  import IconLyrics from "~icons/lucide/mic-vocal";

  const track = $derived(player.track);
  const bg = $derived(coverUrl(track?.coverKey, 64));

  function close() {
    layout.fullscreen = false;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      close();
    }
  }

  function go(r: Parameters<typeof router.go>[0]) {
    close();
    router.go(r);
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="fixed inset-0 z-40 overflow-hidden bg-[#08080b]" transition:fade={{ duration: 220 }}>
  {#key bg}
    <div class="absolute inset-0" transition:fade={{ duration: 900 }}>
      {#if bg}
        <img src={bg} alt="" class="layer layer-a" />
        <img src={bg} alt="" class="layer layer-b" />
        <img src={bg} alt="" class="layer layer-c" />
      {/if}
    </div>
  {/key}
  <div class="absolute inset-0 bg-black/45"></div>
  <div class="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_30%,rgb(0_0_0/0.55)_100%)]"></div>

  <div class="absolute inset-x-0 top-0 h-(--titlebar-h)" data-tauri-drag-region></div>

  <div class="relative flex h-full flex-col px-12 pt-(--titlebar-h) pb-10">
    <div class="flex items-center justify-between">
      <IconButton label="Свернуть" onclick={close}><IconCollapse /></IconButton>
    </div>

    <div class="flex min-h-0 flex-1 items-center justify-center gap-16">
      {#if track}
        <Cover
          coverKey={track.coverKey}
          size={Math.min(460, Math.round(window.innerHeight * 0.5))}
          rounded="lg"
          class="shadow-[0_40px_100px_-30px_rgb(0_0_0/0.9)]"
        />
        <div class="flex w-[min(480px,40vw)] flex-col">
          <h1 class="line-clamp-2 text-[34px] leading-tight font-bold tracking-tight">{track.title}</h1>
          <div class="mt-2 truncate text-[17px] text-ink-2">
            {#each track.artists as a, i (i)}
              {#if i > 0},&nbsp;{/if}
              {#if a.id}
                <button type="button" class="hover:text-ink hover:underline" onclick={() => go({ name: "artist", id: a.id! })}>{a.name}</button>
              {:else}{a.name}{/if}
            {/each}
          </div>
          {#if track.album}
            <button type="button" class="mt-1 self-start truncate text-[14px] text-ink-3 hover:text-ink-2 hover:underline" onclick={() => go({ name: "album", id: track.album!.id })}>
              {track.album.title}
            </button>
          {/if}
          <Timeline class="mt-8" />
          <div class="mt-4 flex justify-center"><Transport big /></div>
          <div class="mt-6 flex items-center justify-center gap-2">
            <IconButton label={t.player.lyrics} size="sm" active={layout.drawer === "lyrics"} onclick={() => layout.toggleDrawer("lyrics")}>
              <IconLyrics />
            </IconButton>
            <IconButton label={t.player.queue} size="sm" onclick={() => { close(); layout.drawer = "queue"; }}>
              <IconQueue />
            </IconButton>
            <Volume />
          </div>
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .layer {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 140vmax;
    height: 140vmax;
    object-fit: cover;
    image-rendering: auto;
    filter: saturate(1.4);
    will-change: transform;
  }
  .layer-a {
    animation: drift-a 38s ease-in-out infinite alternate;
  }
  .layer-b {
    opacity: 0.55;
    mix-blend-mode: screen;
    animation: drift-b 46s ease-in-out infinite alternate;
  }
  .layer-c {
    opacity: 0.4;
    mix-blend-mode: overlay;
    animation: drift-c 31s ease-in-out infinite alternate;
  }
  @keyframes drift-a {
    from { transform: translate(-50%, -50%) rotate(0deg) scale(1.1); }
    to { transform: translate(-46%, -54%) rotate(24deg) scale(1.3); }
  }
  @keyframes drift-b {
    from { transform: translate(-54%, -46%) rotate(180deg) scale(1.2); }
    to { transform: translate(-50%, -50%) rotate(140deg) scale(1); }
  }
  @keyframes drift-c {
    from { transform: translate(-48%, -52%) rotate(90deg) scale(1.4); }
    to { transform: translate(-52%, -48%) rotate(130deg) scale(1.15); }
  }
</style>
