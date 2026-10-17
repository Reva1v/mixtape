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
  import IconGrip from "~icons/lucide/grip-vertical";
  import IconRadio from "~icons/solar/radio-minimalistic-linear";

  let view = $state.raw<QueueView | null>(null);

  $effect(() => {
    void player.queueVersion;
    void player.currentId;
    void invoke<QueueView>("queue_get").then((v) => (view = v));
  });

  const current = $derived(view && view.current != null ? view.items[view.current] : null);
  const upcoming = $derived(view && view.current != null ? view.items.slice(view.current + 1) : (view?.items ?? []));

  // Drag to reorder upcoming items.
  let dragUid = $state<number | null>(null);
  let overIndex = $state<number | null>(null);

  function drop(toUpcomingIndex: number) {
    if (dragUid == null || !view) return;
    const base = (view.current ?? -1) + 1;
    void invoke("queue_move", { uid: dragUid, toIndex: base + toUpcomingIndex });
    dragUid = null;
    overIndex = null;
  }

  function selectChip(id: string) {
    void invoke("mix_select_chip", { chipId: id });
  }
</script>

{#snippet row(e: QueueEntry, i: number, isCurrent: boolean)}
  <div
    role="listitem"
    class="group flex h-14 items-center gap-3 rounded-md px-2 transition-colors hover:bg-fill-2
      {overIndex === i && !isCurrent ? 'shadow-[inset_0_2px_0_white]' : ''}"
    draggable={!isCurrent}
    ondragstart={() => (dragUid = e.uid)}
    ondragover={(ev) => {
      if (isCurrent) return;
      ev.preventDefault();
      overIndex = i;
    }}
    ondragleave={() => (overIndex = null)}
    ondrop={() => drop(i)}
    ondblclick={() => !isCurrent && invoke("queue_jump", { uid: e.uid })}
    oncontextmenu={(ev) => {
      ev.preventDefault();
      menu.show(trackMenu(e.track), ev.clientX, ev.clientY);
    }}
  >
    {#if !isCurrent}
      <IconGrip class="size-4 shrink-0 cursor-grab text-ink-3 opacity-0 group-hover:opacity-100" />
    {/if}
    <Cover coverKey={e.track.coverKey} size={40} rounded="sm" />
    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-2">
        {#if isCurrent}<Equalizer playing={player.playing} />{/if}
        <span class="truncate text-[13.5px] {isCurrent ? 'text-ink' : 'text-ink'}">{e.track.title}</span>
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
  <div class="flex h-14 shrink-0 items-center justify-between px-5">
    <h2 class="text-[16px] font-semibold">{t.queue.title}</h2>
    {#if upcoming.length}
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

  <div class="min-h-0 flex-1 overflow-y-auto px-3 pb-4" role="list">
    {#if !view || view.items.length === 0}
      <p class="px-2 pt-6 text-center text-[13px] text-ink-3">{t.queue.empty}</p>
    {:else}
      {#if current}
        <div class="px-2 pt-1 pb-2 text-[12px] font-semibold tracking-wide text-ink-3 uppercase">{t.queue.nowPlaying}</div>
        {@render row(current, -1, true)}
      {/if}
      {#if upcoming.length}
        <div class="px-2 pt-5 pb-2 text-[12px] font-semibold tracking-wide text-ink-3 uppercase">{t.queue.upNext}</div>
        {#each upcoming as e, i (e.uid)}
          {@render row(e, i, false)}
        {/each}
      {/if}
    {/if}
  </div>
</div>
