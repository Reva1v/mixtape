<script lang="ts">
  // Full-screen player: one compact centred column (cover, title, timeline, transport,
  // volume) over a dark backdrop lit by the cover's colours. With lyrics on, the column
  // moves to the left half and lyrics take the right half.
  //
  // The backdrop is the 64 px cover stretched over the window in slowly drifting layers
  // (`CoverBackdrop`). While the queue sheet covers the player, everything under its blur
  // holds still, so the blur is not recomputed.
  import { setContext } from "svelte";
  import { coverUrl } from "$lib/ipc";
  import { player } from "$lib/stores/player.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { visible } from "$lib/stores/visible.svelte";
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
  import LyricsPanel from "$lib/ui/LyricsPanel.svelte";
  import CoverBackdrop from "$lib/ui/CoverBackdrop.svelte";

  // Equalizers in the queue here are not hidden by this overlay.
  setContext("fullscreen-player", true);

  const track = $derived(player.track);
  const bg = $derived(coverUrl(track?.coverKey, 64));
  const lyrics = $derived(layout.lyrics);

  let height = $state(window.innerHeight);
  const coverSize = $derived(Math.round(Math.min(300, Math.max(200, height * 0.37))));

  function close() {
    layout.fullscreen = false;
  }

  let queueOpen = $state(false);
  let openedAt = 0;
  $effect(() => {
    if (queueOpen) openedAt = performance.now();
  });

  // Guarded "click outside to close": only a short, still click that starts and ends
  // on truly empty space closes the queue. Gaps between cards, the header and the
  // strip just under the last card do not count, nor does the end of a drag.
  let press: { x: number; y: number; t: number } | null = null;

  function isEmpty(e: PointerEvent): boolean {
    const el = e.target as HTMLElement;
    if (el.closest("[role=listitem], button, [role=slider], input, [data-queue-header]")) return false;
    const list = el.closest<HTMLElement>("[role=list]");
    if (!list) return true;
    const rows = list.querySelectorAll<HTMLElement>("[data-row]");
    const last = rows[rows.length - 1];
    return !!last && e.clientY > last.getBoundingClientRect().bottom + 24;
  }

  function queueDown(e: PointerEvent) {
    press = e.button === 0 && isEmpty(e) ? { x: e.clientX, y: e.clientY, t: performance.now() } : null;
  }

  function queueUp(e: PointerEvent) {
    const p = press;
    press = null;
    if (!p || !isEmpty(e)) return;
    const now = performance.now();
    const still = Math.hypot(e.clientX - p.x, e.clientY - p.y) < 6;
    if (still && now - p.t < 500 && now - openedAt > 400) queueOpen = false;
  }

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
      <CoverBackdrop src={bg} running={player.playing && visible.live && !queueOpen} />
    </div>
  {/key}
  <!-- Darken heavily and keep only a soft glow behind the content. -->
  <div class="pointer-events-none absolute inset-0 bg-black/70"></div>
  <div
    class="vignette pointer-events-none absolute inset-0"
    style:--vx={lyrics ? "30%" : "50%"}
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

  <div
    data-tauri-drag-region
    class="relative h-full transition-[filter,opacity] duration-500 ease-out-soft"
    style:filter={queueOpen ? "blur(28px)" : "none"}
    style:opacity={queueOpen ? 0.45 : 1}
  >
    {#if track}
      <!-- Player column -->
      <!-- The column slides from the centre to the middle of the left half with lyrics on;
           only its transform animates. -->
      <section
        data-tauri-drag-region
        class="absolute inset-0 flex flex-col items-center justify-center px-8 transition-transform duration-[650ms] ease-out-soft"
        style:transform="translateX({lyrics ? '-25%' : '0'})"
        in:pop={{ scale: 0.97, y: 18, spring: springs.gentle }}
      >
        <div
          class="relative"
          style:width="{coverSize}px"
          role="img"
          aria-label={track.title}
          oncontextmenu={(e) => {
            e.preventDefault();
            menu.show(trackMenu(track), e.clientX, e.clientY);
          }}
        >
          <Cover coverKey={track.coverKey} size={coverSize} rounded="lg" class="shadow-[0_30px_80px_-24px_rgb(0_0_0/0.95)]" />
          <button
            type="button"
            aria-label="Ещё"
            data-tip="Ещё"
            class="absolute right-2.5 bottom-2.5 grid size-7 place-items-center rounded-full bg-black/45 text-ink backdrop-blur transition-colors hover:bg-black/65"
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

        <Timeline class="mt-6" style="width: {coverSize + 80}px" paused={queueOpen} />

        <div class="mt-5"><Transport variant="full" /></div>

        <div class="mt-6 flex items-center gap-3">
          <Volume class="w-40" />
          <button
            type="button"
            aria-label={t.player.lyrics}
            data-tip={t.player.lyrics}
            class="grid size-7 place-items-center rounded-full transition-colors {lyrics ? 'bg-white/14 text-ink' : 'text-ink-3 hover:text-ink'}"
            onclick={() => layout.toggleLyrics()}
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
        <section
          data-tauri-drag-region
          class="absolute inset-y-0 right-0 w-1/2 min-w-0 pr-16 pl-4"
          transition:pop={{ x: 48, scale: 1, spring: springs.smooth }}
        >
          <LyricsPanel trackId={track.id} paused={queueOpen} />
        </section>
      {/if}
    {/if}
  </div>

  <!-- Queue as a bottom sheet over the player, which blurs out behind it. Clicking any
       empty space (outside rows and controls) closes it. -->
  {#if queueOpen}
    <div class="absolute inset-0 z-20 bg-black/50" transition:fadeSpring></div>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      class="absolute inset-0 z-30 flex justify-center"
      onpointerdown={queueDown}
      onpointerup={queueUp}
    >
      <div class="flex h-full w-full max-w-[720px] flex-col pt-[88px]" transition:sheet>
        <div class="min-h-0 flex-1"><QueuePanel variant="cards" /></div>
      </div>
    </div>
  {/if}
</div>

<style>
  /* Follows the player column; --vx is a registered property, so it animates. */
  .vignette {
    background: radial-gradient(60% 75% at var(--vx) 50%, transparent 0%, rgb(0 0 0 / 0.55) 70%, rgb(0 0 0 / 0.85) 100%);
    transition: --vx 650ms var(--ease-out-soft);
  }
</style>
