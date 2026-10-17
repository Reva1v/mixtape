<script lang="ts">
  // The queue: one list, the playing track highlighted. Virtualized (only the rows in
  // view, plus a margin, exist), so a long queue costs no more than a short one. Rows are
  // dragged with the pointer; the others slide aside; dropping on the playing row plays
  // the dragged track instead.
  import { Virtualizer, type VirtualizerHandle } from "virtua/svelte";
  import type { Attachment } from "svelte/attachments";
  import type { TransitionConfig } from "svelte/transition";
  import { dragScroll, springCss, springs } from "$lib/motion";
  import ArtistLinks from "./ArtistLinks.svelte";
  import { invoke } from "$lib/ipc";
  import type { QueueView } from "$lib/ipc/bindings/QueueView";
  import type { QueueEntry } from "$lib/ipc/bindings/QueueEntry";
  import { player, formatTime } from "$lib/stores/player.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import { trackMenu } from "./trackMenu";
  import Cover from "./Cover.svelte";
  import Equalizer from "./Equalizer.svelte";
  import IconButton from "./IconButton.svelte";
  import IconX from "~icons/solar/close-circle-linear";
  import IconRadio from "~icons/solar/radio-minimalistic-linear";
  import IconStream from "~icons/solar/cloud-linear";
  import IconCached from "~icons/solar/cloud-check-linear";
  import IconLoader from "~icons/lucide/loader-circle";

  /** `cards`: every row is its own translucent card (full-screen player). */
  let { variant = "panel" }: { variant?: "panel" | "cards" } = $props();
  const cards = $derived(variant === "cards");

  let view = $state.raw<QueueView | null>(null);
  let fetchSeq = 0;

  function refetch() {
    const n = ++fetchSeq;
    return invoke<QueueView>("queue_get").then((v) => {
      if (n === fetchSeq) apply(v);
    });
  }

  $effect(() => {
    void player.queueVersion;
    void player.currentId;
    void refetch();
  });

  const items = $derived(view?.items ?? []);
  const currentIndex = $derived(view?.current ?? -1);

  // Building a mix (started anywhere, or another chip picked): from the request until the
  // queue that follows it has arrived, the list keeps what has played and the playing
  // track, and placeholder rows stand in for what comes next.
  let awaiting = $state(false);
  const building = $derived(player.mixPending || awaiting);
  let wasPending = false;
  $effect(() => {
    if (player.mixPending) {
      wasPending = true;
      fading = true;
      scrolledOnce = false;
      return;
    }
    if (!wasPending) return;
    wasPending = false;
    awaiting = true;
    void refetch().finally(() => (awaiting = false));
  });

  const shown = $derived(building ? items.slice(0, currentIndex + 1) : items);
  const hasUpcoming = $derived(!building && items.length > currentIndex + 1);

  // Rows that appear in an update (new mix, another chip, the next page of a mix, tracks
  // added) slide up and fade in, staggered; only rows mounted right then (the ones in
  // view) animate, and never on the panel's first load. Rows a mix replaces fade out.
  const ENTER = springCss(springs.snappy);
  const STAGGER_MS = 35;
  const MAX_STAGGERED = 12;
  let entering = new Set<number>();
  let enterCount = 0;
  let fading = false;
  let settleTimer = 0;
  const reducedMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

  function apply(v: QueueView) {
    if (view && !reducedMotion()) {
      const before = new Set(shown.map((e) => e.uid));
      const fresh = v.items.filter((e) => !before.has(e.uid)).map((e) => e.uid);
      if (fresh.length) {
        entering = new Set(fresh);
        enterCount = 0;
      }
      fading ||= building;
      clearTimeout(settleTimer);
      settleTimer = window.setTimeout(() => {
        entering.clear();
        fading = player.mixPending;
      }, 400);
    }
    view = v;
  }

  function enterRow(uid: number): Attachment<HTMLElement> {
    return (el) => {
      if (!entering.delete(uid)) return;
      const anim = el.animate(
        [
          { opacity: 0, transform: "translateY(10px)" },
          { opacity: 1, transform: "translateY(0)" },
        ],
        { duration: parseFloat(ENTER.ms), easing: ENTER.ease, delay: Math.min(enterCount++, MAX_STAGGERED) * STAGGER_MS, fill: "backwards" },
      );
      return () => anim.cancel();
    };
  }

  function leaveRow(_node: Element): TransitionConfig {
    return fading && !reducedMotion() ? { duration: 140, easing: (t) => t, css: (t) => `opacity:${t}` } : { duration: 0 };
  }

  $effect(() => () => clearTimeout(settleTimer));

  // Drag to reorder queue items: the grabbed row follows the pointer, the others
  // slide aside, and on release it settles into its slot before the move is sent.
  type Drag = {
    uid: number;
    from: number;
    to: number;
    startY: number;
    startScroll: number;
    y: number;
    dy: number;
    step: number;
    active: boolean;
    settling: boolean;
    /** Hovering over the playing track: dropping makes the dragged track play now. */
    replace: boolean;
  };
  let drag = $state<Drag | null>(null);
  let instant = $state(false);
  let listEl = $state<HTMLDivElement>();
  let vlist = $state<VirtualizerHandle>();
  let raf = 0;

  /** Row pitch: 56 px rows, or 64 px cards with an 8 px gap. */
  const ROW = $derived(cards ? 72 : 56);

  function rowStep(i: number): number {
    return vlist?.getItemSize(i) || ROW;
  }

  function down(ev: PointerEvent, i: number, uid: number) {
    if (building || ev.button !== 0 || (ev.target as HTMLElement).closest("button")) return;
    drag = {
      uid,
      from: i,
      to: i,
      startY: ev.clientY,
      startScroll: listEl?.scrollTop ?? 0,
      y: ev.clientY,
      dy: 0,
      step: rowStep(i),
      active: false,
      settling: false,
      replace: false,
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up, { once: true });
  }

  function update() {
    if (!drag || !listEl) return;
    drag.dy = drag.y - drag.startY + (listEl.scrollTop - drag.startScroll);
    const at = drag.from + drag.dy / drag.step;
    drag.replace = currentIndex >= 0 && Math.abs(at - currentIndex) < 0.35;
    drag.to = drag.replace ? drag.from : Math.max(0, Math.min(items.length - 1, drag.from + Math.round(drag.dy / drag.step)));
  }

  function move(ev: PointerEvent) {
    if (!drag || drag.settling) return;
    drag.y = ev.clientY;
    if (!drag.active) {
      if (Math.abs(ev.clientY - drag.startY) < 4) return;
      drag.active = true;
      autoscroll();
    }
    update();
  }

  // Scroll while the pointer is held near the top or bottom edge of the list.
  function autoscroll() {
    cancelAnimationFrame(raf);
    const tick = () => {
      if (!drag?.active || drag.settling || !listEl) return;
      const r = listEl.getBoundingClientRect();
      const edge = 56;
      const speed =
        drag.y < r.top + edge ? -((r.top + edge - drag.y) / edge) * 14 : drag.y > r.bottom - edge ? ((drag.y - r.bottom + edge) / edge) * 14 : 0;
      if (speed) {
        listEl.scrollTop += speed;
        update();
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
  }

  function up() {
    window.removeEventListener("pointermove", move);
    cancelAnimationFrame(raf);
    const d = drag;
    if (!d) return;
    if (d.active && d.replace) {
      playInstead(d);
      return;
    }
    if (!d.active || d.to === d.from) {
      if (d.active) {
        d.settling = true;
        d.dy = 0;
        setTimeout(() => (drag = null), 220);
      } else {
        // A press without movement is a click: play that track.
        drag = null;
        invoke("queue_jump", { uid: d.uid });
      }
      return;
    }
    // Glide into the target slot, then commit without animating the reflow.
    d.settling = true;
    d.dy = (d.to - d.from) * d.step;
    setTimeout(() => {
      if (!view) return;
      const next = [...view.items];
      const [moved] = next.splice(d.from, 1);
      next.splice(d.to, 0, moved);
      // Keep the highlight on the playing track while the core catches up.
      const playingUid = view.current != null ? view.items[view.current].uid : null;
      const current = playingUid == null ? view.current : next.findIndex((x) => x.uid === playingUid);
      instant = true;
      view = { ...view, items: next, current };
      drag = null;
      void invoke("queue_move", { uid: d.uid, toIndex: d.to });
      requestAnimationFrame(() => requestAnimationFrame(() => (instant = false)));
    }, 220);
  }

  // Dropped onto the playing track: glide onto it, place the dragged track right after
  // the old one (which stays in history) and switch to it.
  function playInstead(d: Drag) {
    d.settling = true;
    d.dy = (currentIndex - d.from) * d.step;
    setTimeout(() => {
      if (!view) return;
      const next = [...view.items];
      const [moved] = next.splice(d.from, 1);
      const playingAt = next.findIndex((x) => x.uid === view!.items[currentIndex].uid);
      next.splice(playingAt + 1, 0, moved);
      instant = true;
      view = { ...view, items: next, current: playingAt + 1 };
      drag = null;
      void invoke("queue_move", { uid: d.uid, toIndex: playingAt + 1 }).then(() => invoke("queue_jump", { uid: d.uid }));
      requestAnimationFrame(() => requestAnimationFrame(() => (instant = false)));
    }, 220);
  }

  function rowStyle(i: number): string {
    const d = drag;
    if (!d?.active) return "";
    if (i === d.from) {
      const t = d.settling ? "transition: transform 220ms var(--ease-out-soft);" : "transition: none;";
      return `${t}transform: translateY(${d.dy}px) scale(${d.settling ? 1 : 1.02}); z-index: 10; position: relative;`;
    }
    const shift = d.from < i && i <= d.to ? -d.step : d.to <= i && i < d.from ? d.step : 0;
    return `transform: translateY(${shift}px);`;
  }

  // Bring the playing track into view when the queue opens. Only the list scrolls:
  // scrollIntoView would also scroll every ancestor, including the full-screen player.
  let scrolledOnce = false;
  $effect(() => {
    if (scrolledOnce || !view || !vlist) return;
    scrolledOnce = true;
    const at = currentIndex;
    if (at > 0) queueMicrotask(() => vlist?.scrollToIndex(at, { align: "center" }));
  });

  function selectChip(id: string) {
    void player.selectChip(id);
  }
</script>

{#snippet row(e: QueueEntry, i: number, isCurrent: boolean)}
  <div
    role="listitem"
    data-row=""
    data-current={isCurrent ? "" : undefined}
    class="group flex h-14 items-center gap-3 rounded-md px-2 hover:bg-fill-2
      {instant ? '' : 'transition-[background-color,transform,box-shadow] duration-200 ease-out-soft'}
      {cards ? 'mb-2 h-16 rounded-xl bg-[#0e0e10]/85 px-3 ring-1 ring-white/[0.06] hover:bg-[#18181a]/90' : ''}
      {isCurrent ? (cards ? 'bg-white/[0.13]! ring-white/[0.14]!' : 'bg-fill-2') : ''}
      {isCurrent && drag?.active && drag.replace ? 'ring-2! ring-white/70!' : ''}
      {!isCurrent ? (drag?.active && drag.from === i ? 'cursor-grabbing bg-[#1a1a1c]/90 shadow-[0_16px_40px_-10px_rgb(0_0_0/0.8)]' : 'cursor-grab') : ''}"
    style={rowStyle(i)}
    {@attach enterRow(e.uid)}
    out:leaveRow|global
    onpointerdown={(ev) => !isCurrent && down(ev, i, e.uid)}
    oncontextmenu={(ev) => {
      ev.preventDefault();
      menu.show(trackMenu(e.track), ev.clientX, ev.clientY);
    }}
  >
    <Cover coverKey={e.track.coverKey} size={40} rounded="sm" />
    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-2">
        {#if isCurrent}<Equalizer playing={player.playing} />{/if}
        <span class="truncate text-[13.5px] {isCurrent ? 'font-semibold text-ink' : 'text-ink'}">{e.track.title}</span>
        {#if e.track.state === "remote"}
          <span class="shrink-0 text-ink-3" title="Играет из сети"><IconStream class="size-3.5" /></span>
        {:else if e.track.state === "cached"}
          <span class="shrink-0 text-ink-3" title="В кэше"><IconCached class="size-3.5" /></span>
        {/if}
      </div>
      <div class="truncate text-[12px] text-ink-3"><ArtistLinks artists={e.track.artists} /></div>
    </div>
    {#if isCurrent}
      <span class="text-[12px] text-ink-3 tnum">{formatTime(e.track.durationMs)}</span>
    {:else}
      <span class="text-[12px] text-ink-3 tnum group-hover:hidden">{formatTime(e.track.durationMs)}</span>
      <IconButton label="Убрать из очереди" size="sm" class="hidden group-hover:inline-grid" onclick={() => invoke("queue_remove", { uid: e.uid })}>
        <IconX />
      </IconButton>
    {/if}
  </div>
{/snippet}

<div class="flex h-full flex-col">
  <div class="flex h-14 shrink-0 items-center justify-between px-5" data-queue-header>
    <h2 class="text-[16px] font-semibold">{t.queue.title}</h2>
    {#if hasUpcoming}
      <button type="button" class="text-[12.5px] text-ink-3 hover:text-ink" onclick={() => invoke("queue_clear")}>{t.queue.clear}</button>
    {/if}
  </div>

  {#if view?.chips?.length}
    <div class="flex shrink-0 gap-1.5 overflow-x-auto px-5 pb-3 [scrollbar-width:none]" {@attach dragScroll()}>
      <span class="mr-1 grid shrink-0 place-items-center text-ink-3"><IconRadio class="size-4" /></span>
      {#each view.chips as c (c.id)}
        <button
          type="button"
          class="h-7 shrink-0 rounded-full px-3 text-[12.5px] font-medium transition-colors
            {c.selected ? 'bg-ink text-black' : 'bg-fill-2 text-ink-2 hover:bg-fill-3 hover:text-ink'}"
          onclick={() => selectChip(c.id)}
        >
          {c.title}
        </button>
      {/each}
    </div>
  {/if}

  <div bind:this={listEl} class="min-h-0 flex-1 overflow-y-auto px-3 pb-4 {drag?.active ? 'select-none' : ''}" role="list">
    {#if (!view || items.length === 0) && !building}
      <p class="px-2 pt-6 text-center text-[13px] text-ink-3">{t.queue.empty}</p>
    {:else if listEl}
      <!-- The dragged row stays mounted however far it travels, and above its
           neighbours (each item is its own stacking context). -->
      <Virtualizer
        bind:this={vlist}
        data={shown}
        scrollRef={listEl}
        itemSize={ROW}
        getKey={(e) => e.uid}
        keepMounted={drag?.active ? [drag.from] : undefined}
        itemProps={({ index }) => (drag?.active && index === drag.from ? { style: { "z-index": "10" } } : undefined)}
      >
        {#snippet children(e: QueueEntry, i: number)}
          {@render row(e, i, i === currentIndex)}
        {/snippet}
      </Virtualizer>
      {#if building}
        <div aria-busy="true">
          <div class="flex items-center gap-2 px-2 pt-2 pb-2.5 text-[12.5px] text-ink-3">
            <IconLoader class="size-3.5 animate-spin" />{t.queue.building}
          </div>
          {#each [0.9, 0.7, 0.5, 0.35, 0.2] as fade, k (k)}
            <div
              class="flex h-14 items-center gap-3 px-2 {cards ? 'mb-2 h-16 rounded-xl bg-[#0e0e10]/60 px-3 ring-1 ring-white/[0.04]' : ''}"
              style:opacity={fade}
            >
              <div class="size-10 shrink-0 animate-pulse rounded-sm bg-fill-3" style:animation-delay="{k * 120}ms"></div>
              <div class="min-w-0 flex-1 space-y-2">
                <div class="h-3 animate-pulse rounded bg-fill-3" style:width="{[58, 44, 66, 50, 40][k]}%" style:animation-delay="{k * 120}ms"></div>
                <div class="h-2.5 animate-pulse rounded bg-fill-2" style:width="{[34, 28, 40, 30, 24][k]}%" style:animation-delay="{k * 120}ms"></div>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
</div>
