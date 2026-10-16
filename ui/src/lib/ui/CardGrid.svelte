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
</script>

<div class="px-7" bind:clientWidth={width}>
  {#if layout.scrollEl && width > 0}
    <Virtualizer data={rowIndices} scrollRef={layout.scrollEl} itemSize={cardHeight + GAP} getKey={(r) => r}>
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
