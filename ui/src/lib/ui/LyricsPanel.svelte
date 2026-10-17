<script lang="ts" module>
  import type { Lyrics } from "$lib/ipc/bindings/Lyrics";
  import type { LyricLine } from "$lib/ipc/bindings/LyricLine";

  // Per session: revisiting a track shows its lyrics at once and keeps its offset.
  const loaded = new Map<string, Lyrics | null>();
  const offsets = new Map<string, number>();

  /** Index of the last line that started at or before `ms`, or -1. */
  function lineAt(lines: LyricLine[], ms: number): number {
    let lo = 0;
    let hi = lines.length - 1;
    let found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (lines[mid].ms <= ms) {
        found = mid;
        lo = mid + 1;
      } else hi = mid - 1;
    }
    return found;
  }
</script>

<script lang="ts">
  // Lyrics beside the full-screen player (US5). Synced lines follow playback: the current
  // one is found by binary search every animation frame and springs to ~40 % of the
  // height; scrolling by hand pauses that for a few seconds. Clicking a line seeks.
  import { untrack } from "svelte";
  import { invoke, type IpcError } from "$lib/ipc";
  import { player } from "$lib/stores/player.svelte";
  import { animate, fadeSpring, springs } from "$lib/motion";
  import IconMinus from "~icons/solar/minus-linear";
  import IconPlus from "~icons/solar/add-linear";
  import IconLyrics from "~icons/solar/microphone-2-linear";

  let { trackId }: { trackId: string } = $props();

  type View =
    | { status: "loading" }
    | { status: "ready"; lyrics: Lyrics }
    | { status: "missing" }
    | { status: "error"; message: string };

  let view = $state<View>({ status: "loading" });
  let retry = $state(0);

  const show = (l: Lyrics | null): View => (l ? { status: "ready", lyrics: l } : { status: "missing" });

  $effect(() => {
    const id = trackId;
    void retry;
    const known = loaded.get(id);
    if (known !== undefined) {
      view = show(known);
      return;
    }
    view = { status: "loading" };
    let stale = false;
    invoke<Lyrics | null>("lyrics_get", { trackId: id }).then(
      (l) => {
        loaded.set(id, l);
        if (!stale) view = show(l);
      },
      (e: IpcError) => {
        const message = e.kind === "network" ? "Нет связи с LRCLIB, проверьте интернет" : e.message;
        if (!stale) view = { status: "error", message };
      },
    );
    return () => (stale = true);
  });

  const lyrics = $derived(view.status === "ready" ? view.lyrics : null);
  const lines = $derived(lyrics?.synced ? lyrics.lines : []);

  // Positive = words later. Kept per track for the session.
  let offset = $state(0);
  $effect(() => {
    offset = offsets.get(trackId) ?? 0;
  });
  function shift(ms: number) {
    offset = ms === 0 ? 0 : Math.round((offset + ms) / 100) * 100;
    offsets.set(trackId, offset);
  }
  const offsetLabel = $derived(`${offset > 0 ? "+" : offset < 0 ? "−" : ""}${Math.abs(offset / 1000).toFixed(1)} с`);

  // Current line: per frame while playing, once per update otherwise.
  let active = $state(-1);
  $effect(() => {
    const ls = lines;
    const off = offset;
    void player.state.positionMs;
    void player.state.status;
    if (!ls.length) return;
    let raf = 0;
    const tick = () => {
      const i = lineAt(ls, player.livePosition() - off);
      if (i !== active) active = i;
      if (player.playing) raf = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(raf);
  });

  // Following the current line.
  const PAUSE_MS = 4000;
  let scroller = $state<HTMLElement>();
  let pausedUntil = 0;
  let resumeTimer = 0;
  let scrollAnim: { stop: () => void } | null = null;

  function follow(instant = false) {
    const el = scroller;
    if (!el || performance.now() < pausedUntil) return;
    const row = active >= 0 ? el.querySelector<HTMLElement>(`[data-line="${active}"]`) : null;
    const max = el.scrollHeight - el.clientHeight;
    const target = row ? Math.min(max, Math.max(0, row.offsetTop + row.offsetHeight / 2 - el.clientHeight * 0.4)) : 0;
    scrollAnim?.stop();
    if (instant) {
      el.scrollTop = target;
      return;
    }
    scrollAnim = animate(el.scrollTop, target, {
      type: "spring",
      ...springs.gentle,
      onUpdate: (v: number) => (el.scrollTop = v),
    });
  }

  $effect(() => {
    void active;
    untrack(() => follow());
  });

  // New lyrics land already on the current line.
  $effect(() => {
    if (!lines.length || !scroller) return;
    untrack(() => {
      pausedUntil = 0;
      follow(true);
    });
  });

  /** Wheel, drag or keys on the list: stop following for a while. */
  function userScroll() {
    scrollAnim?.stop();
    pausedUntil = performance.now() + PAUSE_MS;
    clearTimeout(resumeTimer);
    resumeTimer = window.setTimeout(() => follow(), PAUSE_MS + 20);
  }

  function seekTo(line: LyricLine) {
    void player.seek(Math.max(0, line.ms + offset));
    pausedUntil = 0;
    clearTimeout(resumeTimer);
    follow();
  }

  $effect(() => () => {
    clearTimeout(resumeTimer);
    scrollAnim?.stop();
  });
</script>

<div class="relative h-full min-w-0">
  {#if lyrics && !lyrics.instrumental}
    <!-- Provider and offset. No backdrop blur: the panel fades in, and WebView2 blinks
         blurred layers whose opacity animates. -->
    <div class="absolute top-[18px] left-1/2 z-10 flex h-8 -translate-x-1/2 items-center gap-0.5 rounded-lg bg-black/45 px-[3px] text-[12px] text-ink ring-1 ring-line">
      <span class="px-2 font-medium tracking-wide">LRCLIB</span>
      {#if lyrics.synced}
        <span class="mx-0.5 h-3.5 w-px bg-white/15"></span>
        <button
          type="button"
          aria-label="Текст раньше на 0,1 с"
          data-tip="Раньше на 0,1 с"
          data-tip-pos="bottom"
          class="grid size-[26px] place-items-center rounded-md text-ink-2 transition-colors hover:bg-white/10 hover:text-ink"
          onclick={() => shift(-100)}
        >
          <IconMinus class="size-3.5" />
        </button>
        <button
          type="button"
          data-tip={offset ? "Сдвиг текста · сбросить" : "Сдвиг текста"}
          data-tip-pos="bottom"
          class="h-[26px] min-w-[52px] rounded-md px-1.5 tnum transition-colors hover:bg-white/10
            {offset ? 'font-semibold text-ink' : 'text-ink-2'}"
          onclick={() => shift(0)}
        >
          {offsetLabel}
        </button>
        <button
          type="button"
          aria-label="Текст позже на 0,1 с"
          data-tip="Позже на 0,1 с"
          data-tip-pos="bottom"
          class="grid size-[26px] place-items-center rounded-md text-ink-2 transition-colors hover:bg-white/10 hover:text-ink"
          onclick={() => shift(100)}
        >
          <IconPlus class="size-3.5" />
        </button>
      {:else}
        <span class="pr-2 text-ink-3">без синхронизации</span>
      {/if}
    </div>
  {/if}

  {#if view.status === "loading"}
    <div class="flex h-full flex-col justify-center gap-6" in:fadeSpring>
      {#each [72, 58, 84, 46, 66, 78, 52] as w, i (i)}
        <div class="h-7 animate-pulse rounded-md bg-fill-2" style:width="{w}%" style:animation-delay="{i * 90}ms"></div>
      {/each}
    </div>
  {:else if view.status === "ready" && view.lyrics.instrumental}
    {@render calm("Инструментал")}
  {:else if view.status === "ready" && view.lyrics.synced}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      bind:this={scroller}
      class="scroller h-full overflow-y-auto"
      onwheel={userScroll}
      onpointerdown={(e) => e.target === scroller && userScroll()}
      onkeydown={userScroll}
      ontouchmove={userScroll}
      in:fadeSpring
    >
      {#each view.lyrics.lines as line, i (i)}
        <button
          type="button"
          data-line={i}
          class="line block w-full cursor-pointer py-[7px] text-left text-[30px] leading-[1.25] font-bold text-pretty text-ink"
          class:active={i === active}
          onclick={() => seekTo(line)}
        >
          {line.text || "• • •"}
        </button>
      {/each}
    </div>
  {:else if view.status === "ready"}
    <div class="scroller plain h-full overflow-y-auto" in:fadeSpring>
      <p class="text-[24px] leading-[1.45] font-bold whitespace-pre-line text-ink-2 select-text">{view.lyrics.plain}</p>
    </div>
  {:else if view.status === "missing"}
    {@render calm("Текст не найден")}
  {:else}
    {@render calm("Не удалось загрузить текст", view.message)}
  {/if}
</div>

{#snippet calm(title: string, hint?: string)}
  <div class="flex h-full flex-col items-center justify-center gap-3 text-center text-ink-3" in:fadeSpring>
    <IconLyrics class="size-9 opacity-60" />
    <p class="text-[15px]">{title}</p>
    {#if hint}
      <p class="max-w-xs text-[12.5px] leading-relaxed opacity-80">{hint}</p>
      <button
        type="button"
        class="mt-1 rounded-full bg-white/8 px-3.5 py-1.5 text-[13px] text-ink transition-colors hover:bg-white/14"
        onclick={() => retry++}
      >
        Повторить
      </button>
    {/if}
  </div>
{/snippet}

<style>
  .scroller {
    position: relative;
    scrollbar-width: none;
    padding-block: 40vh 50vh;
    mask-image: linear-gradient(180deg, transparent 0, transparent 7%, #000 22%, #000 78%, transparent 100%);
  }
  .scroller.plain {
    padding-block: 80px 30vh;
    mask-image: linear-gradient(180deg, #000 85%, transparent 100%);
  }
  /* Only opacity changes: dimmed lines, the current one at full white. */
  .line {
    opacity: 0.3;
    transition: opacity 420ms var(--ease-out-soft);
  }
  .line:hover {
    opacity: 0.6;
  }
  .line.active {
    opacity: 1;
  }
</style>
