<script lang="ts">
  // Full-screen player: one compact centred column (cover, title, timeline, transport,
  // volume) over a dark backdrop lit by the cover's colours. With lyrics on, the column
  // moves to the left half and lyrics take the right half.
  //
  // The backdrop is the 64 px cover stretched over the window in slowly drifting layers:
  // upscaling blurs it for free and only transform/opacity animate.
  import { coverUrl } from "$lib/ipc";
  import { player } from "$lib/stores/player.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n/ru";
  import { fade } from "svelte/transition";
  import { fadeSpring, pop, sheet, springs } from "$lib/motion";
  import { trackMenu } from "$lib/ui/trackMenu";
  import Cover from "$lib/ui/Cover.svelte";
  import Transport from "$lib/ui/Transport.svelte";
  import Timeline from "$lib/ui/Timeline.svelte";
  import Volume from "$lib/ui/Volume.svelte";
  import IconCollapse from "~icons/solar/alt-arrow-down-linear";
  import IconPlus from "~icons/lucide/plus";
  import IconLyrics from "~icons/solar/microphone-2-linear";
  import IconQueue from "~icons/solar/playlist-minimalistic-linear";
  import QueuePanel from "$lib/ui/QueuePanel.svelte";

  const track = $derived(player.track);
  const bg = $derived(coverUrl(track?.coverKey, 64));
  const lyrics = $derived(layout.drawer === "lyrics");

  let height = $state(window.innerHeight);
  const coverSize = $derived(Math.round(Math.min(300, Math.max(200, height * 0.37))));

  function close() {
    layout.fullscreen = false;
  }

  let queueOpen = $state(false);

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      if (queueOpen) queueOpen = false;
      else close();
    }
  }

  function go(r: Parameters<typeof router.go>[0]) {
    close();
    router.go(r);
  }
</script>

<svelte:window onkeydown={onKey} bind:innerHeight={height} />

<div class="fixed inset-0 z-40 overflow-hidden bg-[#070709]" transition:fadeSpring data-tauri-drag-region>
  {#key bg}
    <div class="pointer-events-none absolute inset-0" transition:fade={{ duration: prefs.value.smoothBackground ? 1400 : 0 }}>
      {#if bg}
        <img src={bg} alt="" class="layer layer-a" />
        <img src={bg} alt="" class="layer layer-b" />
      {/if}
    </div>
  {/key}
  <!-- Darken heavily and keep only a soft glow behind the content. -->
  <div class="pointer-events-none absolute inset-0 bg-black/70"></div>
  <div
    class="pointer-events-none absolute inset-0"
    style:background="radial-gradient(60% 75% at {lyrics ? '30%' : '50%'} 50%, transparent 0%, rgb(0 0 0 / 0.55) 70%, rgb(0 0 0 / 0.85) 100%)"
  ></div>


  <button
    type="button"
    aria-label="Свернуть"
    data-tip="Свернуть (Esc)"
    data-tip-pos="right"
    class="absolute top-4 left-4 z-10 grid size-9 place-items-center rounded-full bg-white/8 text-ink-2 backdrop-blur
      transition-colors hover:bg-white/14 hover:text-ink"
    onclick={close}
  >
    <IconCollapse class="size-[18px]" />
  </button>

  <div data-tauri-drag-region class="relative grid h-full transition-[grid-template-columns] duration-500 ease-out-soft {lyrics ? 'grid-cols-2' : 'grid-cols-1'}">
    {#if track}
      <!-- Player column -->
      <section data-tauri-drag-region class="flex flex-col items-center justify-center px-8" style:width="100%" in:pop={{ scale: 0.97, y: 18, spring: springs.gentle }}>
        <div class="relative" style:width="{coverSize}px">
          <Cover coverKey={track.coverKey} size={coverSize} rounded="lg" class="shadow-[0_30px_80px_-24px_rgb(0_0_0/0.95)]" />
          <button
            type="button"
            aria-label="Ещё"
            class="absolute bottom-2.5 left-2.5 grid size-7 place-items-center rounded-full bg-black/45 text-ink backdrop-blur transition-colors hover:bg-black/65"
            onclick={(e) => menu.showAt(trackMenu(track), e.currentTarget)}
          >
            <IconPlus class="size-4" />
          </button>
        </div>

        <div class="mt-6 text-center" style:width="{coverSize + 40}px">
          <div class="truncate text-[15px] font-semibold text-ink">{track.title}</div>
          <div class="mt-0.5 truncate text-[13px] text-ink-2">
            {#each track.artists as a, i (i)}
              {#if i > 0},&nbsp;{/if}
              {#if a.id}
                <button type="button" class="hover:text-ink hover:underline" onclick={() => go({ name: "artist", id: a.id! })}>{a.name}</button>
              {:else}{a.name}{/if}
            {/each}
          </div>
        </div>

        <Timeline class="mt-6" style="width: {coverSize + 80}px" />

        <div class="mt-5"><Transport variant="full" /></div>

        <div class="mt-6 flex items-center gap-3">
          <Volume class="w-40" />
          <button
            type="button"
            aria-label={t.player.lyrics}
            data-tip={t.player.lyrics}
            class="grid size-7 place-items-center rounded-full transition-colors {lyrics ? 'bg-white/14 text-ink' : 'text-ink-3 hover:text-ink'}"
            onclick={() => layout.toggleDrawer("lyrics")}
          >
            <IconLyrics class="size-4" />
          </button>
          <button
            type="button"
            aria-label={t.player.queue}
            data-tip={t.player.queue}
            class="grid size-7 place-items-center rounded-full transition-colors {queueOpen ? 'bg-white/14 text-ink' : 'text-ink-3 hover:text-ink'}"
            onclick={() => (queueOpen = !queueOpen)}
          >
            <IconQueue class="size-4" />
          </button>
        </div>
      </section>

      {#if lyrics}
        <!-- Lyrics column (US5). -->
        <section data-tauri-drag-region class="flex min-w-0 items-center pr-16 pl-4" transition:fade={{ duration: 250 }}>
          <p class="text-[30px] leading-snug font-bold text-ink-3">Тексты песен появятся позже</p>
        </section>
      {/if}
    {/if}
  </div>

  <!-- Queue as a bottom sheet over the player, which blurs out behind it. Clicking any
       empty space (outside rows and controls) closes it. -->
  {#if queueOpen}
    <div class="absolute inset-0 z-20 bg-black/55 backdrop-blur-[40px]" transition:fadeSpring></div>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      class="absolute inset-0 z-30 flex justify-center"
      onclick={(e) => {
        if (!(e.target as HTMLElement).closest("[role=listitem], button, [role=slider], input")) queueOpen = false;
      }}
    >
      <div class="flex h-[80%] w-full max-w-[720px] flex-col self-end" transition:sheet>
        <div class="min-h-0 flex-1"><QueuePanel variant="cards" /></div>
      </div>
    </div>
  {/if}
</div>

<style>
  .layer {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 130vmax;
    height: 130vmax;
    object-fit: cover;
    filter: saturate(1.6);
    will-change: transform;
  }
  .layer-a {
    animation: drift-a 40s ease-in-out infinite alternate;
  }
  .layer-b {
    opacity: 0.5;
    mix-blend-mode: screen;
    animation: drift-b 52s ease-in-out infinite alternate;
  }
  @keyframes drift-a {
    from { transform: translate(-50%, -50%) rotate(0deg) scale(1.1); }
    to { transform: translate(-47%, -53%) rotate(20deg) scale(1.25); }
  }
  @keyframes drift-b {
    from { transform: translate(-53%, -47%) rotate(170deg) scale(1.2); }
    to { transform: translate(-50%, -50%) rotate(140deg) scale(1.05); }
  }
</style>
