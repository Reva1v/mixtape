<script lang="ts">
  // «Распознать трек» (specs/002): opening it starts listening, closing it (button, Esc) frees
  // the device at once. Shazam names the song, YouTube Music gives the track to play. The
  // source is the computer's sound or the microphone; the choice is remembered on this PC.
  import { untrack } from "svelte";
  import { coverUrl, invoke } from "$lib/ipc";
  import { pop } from "$lib/motion";
  import type { CaptureSource } from "$lib/ipc/bindings/CaptureSource";
  import type { RecognizeResult } from "$lib/ipc/bindings/RecognizeResult";
  import type { RecognizedSong } from "$lib/ipc/bindings/RecognizedSong";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import Cover from "./Cover.svelte";
  import Modal from "./Modal.svelte";
  import IconWave from "~icons/solar/soundwave-linear";
  import IconPlay from "~icons/solar/play-bold";
  import IconAlbum from "~icons/solar/vinyl-linear";
  import IconArtist from "~icons/solar/user-linear";
  import IconAgain from "~icons/solar/restart-linear";
  import IconSearch from "~icons/solar/magnifier-linear";
  import IconMic from "~icons/solar/microphone-linear";
  import IconComputer from "~icons/solar/monitor-linear";
  import IconOffline from "~icons/solar/cloud-cross-linear";
  import IconProblem from "~icons/solar/danger-circle-linear";
  import IconNothing from "~icons/solar/close-circle-linear";

  interface Props {
    open: boolean;
    onclose: () => void;
    /** «Искать в YouTube Music»: the search screen takes the query. */
    onsearch: (query: string) => void;
  }
  let { open, onclose, onsearch }: Props = $props();

  type Phase =
    "listening" | "found" | "notFound" | "denied" | "noDevice" | "offline" | "limited" | "failed";
  const SOURCE_KEY = "recognize-source";

  let phase = $state<Phase>("listening");
  let source = $state<CaptureSource>(
    localStorage.getItem(SOURCE_KEY) === "microphone" ? "microphone" : "computer",
  );
  let song = $state<RecognizedSong | null>(null);
  let track = $state<Track | null>(null);
  let failure = $state("");
  /** Each listen's number: an answer to an older one is dropped. */
  let run = 0;

  $effect(() => {
    if (open) untrack(() => void listen());
    else untrack(stop);
  });

  async function listen() {
    const mine = ++run;
    phase = "listening";
    song = null;
    track = null;
    failure = "";
    try {
      const r = await invoke<RecognizeResult>("recognize_start", { source });
      if (mine !== run || r.status === "cancelled") return;
      song = r.song ?? null;
      track = r.track ?? null;
      phase = r.status === "found" && !r.song ? "notFound" : r.status;
    } catch (e) {
      if (mine !== run) return;
      phase = "failed";
      failure = (e as Error).message;
    }
  }

  function stop() {
    run++;
    if (phase === "listening") void invoke("recognize_cancel").catch(() => {});
  }

  function choose(next: CaptureSource) {
    if (next === source) return;
    source = next;
    localStorage.setItem(SOURCE_KEY, next);
    void listen();
  }

  /** Closes the sheet, then does what was asked. */
  async function then(f: () => void | Promise<unknown>) {
    onclose();
    try {
      await f();
    } catch (e) {
      toasts.push((e as Error).message, "error");
    }
  }

  // Live bars around the button, newest at the top, flowing down both sides. Rust does not push
  // these: the screen asks 10 times a second while it listens.
  const BARS = 48;
  const HALF = BARS / 2;
  /** Each bar's own little variation, so the circle doesn't look stamped. */
  const shape = Array.from({ length: BARS }, (_, i) => 0.7 + 0.3 * Math.abs(Math.sin(i * 2.3)));
  let levels = $state<number[]>(Array(HALF).fill(0));
  /** Smoothed loudness: the button breathes with it. */
  let energy = $state(0);
  let quietSince = $state(0);
  let now = $state(0);
  const quiet = $derived(quietSince > 0 && now - quietSince > 2500);
  const barScale = (i: number) =>
    0.12 + 0.88 * (levels[i < HALF ? i : BARS - 1 - i] ?? 0) * shape[i];

  $effect(() => {
    if (!open || phase !== "listening") return;
    levels = Array(HALF).fill(0);
    energy = 0;
    quietSince = performance.now();
    let pending = false;
    const id = setInterval(async () => {
      now = performance.now();
      if (pending || document.hidden) return;
      pending = true;
      try {
        const v = await invoke<number>("recognize_level");
        levels = [v, ...levels.slice(0, HALF - 1)];
        energy = energy * 0.55 + v * 0.45;
        if (energy > 0.18) quietSince = 0;
        else if (!quietSince) quietSince = performance.now();
      } catch {
        // No level this time: the bars just stay.
      } finally {
        pending = false;
      }
    }, 100);
    return () => clearInterval(id);
  });

  const artistId = $derived(track?.artists.find((a) => a.id)?.id);
  const albumId = $derived(track?.album?.id);
  const glow = $derived(track?.coverKey ? coverUrl(track.coverKey, 300) : song?.coverUrl);
  const hint = $derived(
    source === "computer" ? t.recognize.computerHint : t.recognize.microphoneHint,
  );
  const quietHint = $derived(
    source === "computer" ? t.recognize.computerQuiet : t.recognize.microphoneQuiet,
  );
</script>

{#snippet problem(
  icon: typeof IconWave,
  title: string,
  text: string,
  second?: { label: string; run: () => void },
)}
  {@const Icon = icon}
  <div class="flex flex-col items-center py-8 text-center" in:pop={{ y: 8 }}>
    <span class="flex size-16 items-center justify-center rounded-full bg-fill-2 text-ink-2"
      ><Icon class="size-7" /></span
    >
    <p class="mt-5 text-[18px] font-bold">{title}</p>
    <p class="mt-2 max-w-[340px] text-[13px] leading-snug text-ink-3">{text}</p>
    <div class="mt-6 flex gap-2">
      {#if second}
        <button
          type="button"
          class="h-9 rounded-full bg-fill-2 px-4 text-[13px] font-semibold hover:bg-fill-3"
          onclick={second.run}>{second.label}</button
        >
      {/if}
      <button
        type="button"
        class="h-9 rounded-full bg-ink px-4 text-[13px] font-semibold text-black"
        onclick={listen}>{t.recognize.again}</button
      >
    </div>
  </div>
{/snippet}

<Modal {open} title={t.recognize.title} {onclose} width={460}>
  {#if open}
    {#if phase !== "found"}
      <div
        class="mb-2 grid grid-cols-2 gap-1 rounded-full bg-fill-1 p-1"
        role="radiogroup"
        aria-label={t.recognize.source}
      >
        {#each [["computer", t.recognize.computer, IconComputer], ["microphone", t.recognize.microphone, IconMic]] as const as [id, label, Icon] (id)}
          <button
            type="button"
            role="radio"
            aria-checked={source === id}
            class="flex h-8 items-center justify-center gap-1.5 rounded-full text-[13px] font-medium transition-colors {source ===
            id
              ? 'bg-fill-3 text-ink'
              : 'text-ink-3 hover:text-ink-2'}"
            onclick={() => choose(id)}
          >
            <Icon class="size-4" />{label}
          </button>
        {/each}
      </div>
    {/if}

    {#if phase === "listening"}
      <div class="flex flex-col items-center" in:pop>
        <div class="listen relative flex size-64 items-center justify-center" style:--lvl={energy}>
          <span class="ring"></span>
          <span class="ring" style="animation-delay: 0.6s"></span>
          <span class="ring" style="animation-delay: 1.2s"></span>
          {#each shape as _, i (i)}
            <span
              class="bar"
              style:transform="rotate({(i * 360) / BARS}deg) translateY(-64px) scaleY({barScale(
                i,
              )})"
            ></span>
          {/each}
          <span
            class="core relative flex size-28 items-center justify-center rounded-full bg-ink text-black"
            style:scale={1 + energy * 0.14}
          >
            <IconWave class="size-12" />
          </span>
        </div>
        <p class="text-[18px] font-bold" aria-live="polite">{t.recognize.listening}</p>
        <p class="mt-1.5 text-center text-[13px] {quiet ? 'text-ink' : 'text-ink-3'}">
          {quiet ? quietHint : hint}
        </p>
        <button
          type="button"
          class="mt-5 mb-2 h-9 rounded-full bg-fill-2 px-5 text-[13px] font-semibold hover:bg-fill-3"
          onclick={onclose}>{t.recognize.cancel}</button
        >
      </div>
    {:else if phase === "found" && song}
      {@const s = song}
      <div class="relative flex flex-col items-center overflow-hidden pt-1 pb-2" in:pop={{ y: 10 }}>
        {#if glow}<div
            aria-hidden="true"
            class="glow"
            style:background-image="url({glow})"
          ></div>{/if}
        {#if track?.coverKey}
          <Cover coverKey={track.coverKey} size={220} rounded="lg" class="relative size-[220px]" />
        {:else if s.coverUrl}
          <img src={s.coverUrl} alt="" class="relative size-[220px] rounded-xl object-cover" />
        {:else}
          <Cover size={220} rounded="lg" class="relative size-[220px]" />
        {/if}
        <p
          class="relative mt-5 w-full truncate text-center text-[22px] font-extrabold tracking-tight"
        >
          {s.title}
        </p>
        <p class="relative mt-0.5 w-full truncate text-center text-[14.5px] text-ink-2">
          {s.artist}
        </p>
        {#if s.album}<p
            class="relative mt-0.5 w-full truncate text-center text-[12.5px] text-ink-3"
          >
            {s.album}
          </p>{/if}

        <div class="relative mt-6 flex w-full flex-col gap-2">
          {#if track}
            {@const tr = track}
            <button
              type="button"
              class="action bg-ink text-black"
              onclick={() => then(() => player.mix(tr.id))}
            >
              <IconPlay class="size-4" />{t.recognize.play}
            </button>
            <div class="grid grid-cols-2 gap-2">
              <button
                type="button"
                class="action bg-fill-2 hover:bg-fill-3 disabled:opacity-40"
                disabled={!albumId}
                onclick={() => albumId && then(() => router.go({ name: "album", id: albumId }))}
              >
                <IconAlbum class="size-4" />{t.recognize.album}
              </button>
              <button
                type="button"
                class="action bg-fill-2 hover:bg-fill-3 disabled:opacity-40"
                disabled={!artistId}
                onclick={() => artistId && then(() => router.go({ name: "artist", id: artistId }))}
              >
                <IconArtist class="size-4" />{t.recognize.artist}
              </button>
            </div>
          {:else}
            <p class="pb-1 text-center text-[12.5px] text-ink-3">{t.recognize.notOnYouTube}</p>
            <button
              type="button"
              class="action bg-ink text-black"
              onclick={() => then(() => onsearch(`${s.artist} ${s.title}`.trim()))}
            >
              <IconSearch class="size-4" />{t.recognize.searchYouTube}
            </button>
          {/if}
          <button type="button" class="action text-ink-2 hover:text-ink" onclick={listen}>
            <IconAgain class="size-4" />{t.recognize.again}
          </button>
        </div>
      </div>
    {:else if phase === "denied"}
      {@render problem(IconMic, t.recognize.denied, t.recognize.deniedHint, {
        label: t.recognize.settings,
        run: () => void invoke("recognize_settings").catch(() => {}),
      })}
    {:else if phase === "noDevice"}
      {@render problem(
        source === "computer" ? IconComputer : IconMic,
        t.recognize.noDevice,
        source === "computer" ? t.recognize.noOutputHint : t.recognize.noMicHint,
      )}
    {:else if phase === "offline"}
      {@render problem(IconOffline, t.recognize.offline, t.recognize.offlineHint)}
    {:else if phase === "limited"}
      {@render problem(IconProblem, t.recognize.limited, t.recognize.limitedHint)}
    {:else if phase === "failed"}
      {@render problem(IconProblem, t.recognize.failed, failure)}
    {:else}
      {@render problem(
        IconNothing,
        t.recognize.nothing,
        source === "computer" ? t.recognize.nothingComputer : t.recognize.nothingMicrophone,
      )}
    {/if}
  {/if}
</Modal>

<style>
  .glow {
    position: absolute;
    inset: -10% -20% auto;
    height: 70%;
    background-size: cover;
    background-position: center;
    filter: blur(50px) saturate(1.4);
    opacity: 0.3;
    pointer-events: none;
  }
  .ring {
    position: absolute;
    inset: 72px;
    border-radius: 999px;
    /* Louder music, brighter rings. */
    background: rgb(255 255 255 / calc(0.08 + var(--lvl, 0) * 0.22));
    animation: spread 1.8s ease-out infinite;
  }
  .core {
    transition: scale 120ms ease-out;
  }
  /* One bar of the circle: its foot on the centre, pushed out past the button and stretched
     by the loudness. */
  .bar {
    position: absolute;
    left: calc(50% - 2px);
    top: calc(50% - 30px);
    width: 4px;
    height: 30px;
    border-radius: 2px;
    background: var(--color-ink, #fff);
    opacity: 0.85;
    transform-origin: 50% 100%;
    transition: transform 110ms linear;
  }
  @keyframes spread {
    from {
      transform: scale(1);
      opacity: 0.9;
    }
    to {
      transform: scale(2);
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ring {
      animation: none;
    }
    .bar,
    .core {
      transition: none;
    }
  }
  .action {
    display: flex;
    height: 40px;
    align-items: center;
    justify-content: center;
    gap: 7px;
    border-radius: 999px;
    font-size: 13.5px;
    font-weight: 600;
    transition: background-color 120ms;
  }
</style>
