<script lang="ts">
  import { invoke } from "$lib/ipc";
  import type { QueueView } from "$lib/ipc/bindings/QueueView";
  import type { QueueEntry } from "$lib/ipc/bindings/QueueEntry";
  import { player, formatTime } from "$lib/stores/player.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n/ru";
  import { trackMenu } from "./trackMenu";
  import Cover from "./Cover.svelte";
  import Equalizer from "./Equalizer.svelte";
  import IconButton from "./IconButton.svelte";
  import IconX from "~icons/solar/close-circle-linear";
  import IconRadio from "~icons/solar/radio-minimalistic-linear";

  /** `cards`: every row is its own translucent card (full-screen player). */
  let { variant = "panel" }: { variant?: "panel" | "cards" } = $props();
  const cards = $derived(variant === "cards");

  let view = $state.raw<QueueView | null>(null);

  $effect(() => {
    void player.queueVersion;
    void player.currentId;
    void invoke<QueueView>("queue_get").then((v) => (view = v));
  });

  const items = $derived(view?.items ?? []);
  const currentIndex = $derived(view?.current ?? -1);
  const hasUpcoming = $derived(items.length > currentIndex + 1);

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
  let upEl = $state<HTMLDivElement>();
  let raf = 0;

  function rowStep(): number {
    const rows = upEl?.querySelectorAll<HTMLElement>("[data-row]");
    if (!rows?.length) return 56;
    if (rows.length > 1) return rows[1].offsetTop - rows[0].offsetTop;
    const cs = getComputedStyle(rows[0]);
    return rows[0].offsetHeight + parseFloat(cs.marginBottom);
  }

  function down(ev: PointerEvent, i: number, uid: number) {
    if (ev.button !== 0 || (ev.target as HTMLElement).closest("button")) return;
    drag = {
      uid,
      from: i,
      to: i,
      startY: ev.clientY,
      startScroll: listEl?.scrollTop ?? 0,
      y: ev.clientY,
      dy: 0,
      step: rowStep(),
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
      } else drag = null;
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

  // Bring the playing track into view when the queue opens.
  let scrolledOnce = false;
  $effect(() => {
    if (scrolledOnce || !view || !upEl) return;
    scrolledOnce = true;
    // Scroll only the list: scrollIntoView would also scroll every ancestor, including
    // the full-screen player, and shift it under its fixed background.
    queueMicrotask(() => {
      const row = upEl?.querySelector<HTMLElement>("[data-current]");
      if (!row || !listEl) return;
      const top = row.getBoundingClientRect().top - listEl.getBoundingClientRect().top + listEl.scrollTop;
      listEl.scrollTop = top - listEl.clientHeight / 2 + row.offsetHeight / 2;
    });
  });

  function selectChip(id: string) {
    void invoke("mix_select_chip", { chipId: id });
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
    onpointerdown={(ev) => !isCurrent && down(ev, i, e.uid)}
    ondblclick={() => !isCurrent && invoke("queue_jump", { uid: e.uid })}
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
      </div>
      <div class="truncate text-[12px] text-ink-3">{e.track.artists.map((a) => a.name).join(", ")}</div>
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
    <div class="flex shrink-0 gap-1.5 overflow-x-auto px-5 pb-3 [scrollbar-width:none]">
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
    {#if !view || items.length === 0}
      <p class="px-2 pt-6 text-center text-[13px] text-ink-3">{t.queue.empty}</p>
    {:else}
      <div bind:this={upEl}>
        {#each items as e, i (e.uid)}
          {@render row(e, i, i === currentIndex)}
        {/each}
      </div>
    {/if}
  </div>
</div>
