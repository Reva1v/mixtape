<script lang="ts" generics="T">
  // Virtualized grid of cards: rows of as many columns as fit, scrolled by the page.
  import type { Snippet } from "svelte";
  import { Virtualizer } from "virtua/svelte";
  import type { RowSource } from "$lib/stores/paged.svelte";
  import { layout } from "$lib/stores/layout.svelte";

  interface Props {
    rows: RowSource<T>;
    card: Snippet<[item: T | undefined, index: number]>;
    minWidth?: number;
    /** Card height in px including the caption, used for row size. */
    cardHeight: number;
  }

  let { rows, card, minWidth = 170, cardHeight }: Props = $props();

  const GAP = 20;
  let width = $state(0);
  const cols = $derived(Math.max(2, Math.floor((width + GAP) / (minWidth + GAP))));
  const rowIndices = $derived(Array.from({ length: Math.ceil(rows.count / cols) }, (_, i) => i));

  // Where the grid starts inside the scroll content (see TrackList).
  let host = $state<HTMLDivElement>();
  let startMargin = $state(0);
  $effect(() => {
    const scroller = layout.scrollEl;
    if (!host || !scroller) return;
    const measure = () => {
      startMargin = Math.round(
        host!.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop,
      );
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(scroller.firstElementChild ?? scroller);
    return () => ro.disconnect();
  });
</script>

<div class="px-8" bind:clientWidth={width} bind:this={host}>
  {#if layout.scrollEl && width > 0}
    <Virtualizer data={rowIndices} scrollRef={layout.scrollEl} itemSize={cardHeight + GAP} {startMargin} getKey={(r) => r}>
      {#snippet children(r: number)}
        <div class="grid pb-5" style:grid-template-columns="repeat({cols}, minmax(0, 1fr))" style:column-gap="{GAP}px">
          {#each { length: cols } as _, c (c)}
            {@const i = r * cols + c}
            {#if i < rows.count}
              {@render card(rows.get(i), i)}
            {/if}
          {/each}
        </div>
      {/snippet}
    </Virtualizer>
  {/if}
</div>
