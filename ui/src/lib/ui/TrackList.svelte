<script lang="ts">
  // Track list that scrolls with the page (scrollRef = main area), so headers above it
  // scroll away naturally. Short lists render every row; long ones are virtualized and
  // told where they start inside the scroll container, so the visible range is right
  // even when the list sits halfway down a page.
  import { Virtualizer } from "virtua/svelte";
  import type { Track } from "$lib/ipc/bindings/Track";
  import type { RowSource } from "$lib/stores/paged.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import TrackRow from "./TrackRow.svelte";

  interface Props {
    rows: RowSource<Track>;
    onplay: (index: number) => void;
    showCover?: boolean;
    showAlbum?: boolean;
    /** Use track numbers (album page). */
    numbered?: boolean;
  }

  let { rows, onplay, showCover = true, showAlbum = true, numbered = false }: Props = $props();

  /** Below this many rows the DOM cost is negligible and plain rendering never flickers. */
  const VIRTUAL_FROM = 150;

  const indices = $derived(Array.from({ length: rows.count }, (_, i) => i));
  const virtual = $derived(rows.count > VIRTUAL_FROM);

  let host = $state<HTMLDivElement>();
  let startMargin = $state(0);

  // Offset of the list from the top of the scroll content; re-measured when the
  // content above it changes size.
  $effect(() => {
    const scroller = layout.scrollEl;
    if (!virtual || !host || !scroller) return;
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

{#snippet row(i: number)}
  {@const track = rows.get(i)}
  <TrackRow
    {track}
    index={i}
    number={numbered ? (track?.trackNo ?? null) : null}
    {showCover}
    {showAlbum}
    onplay={() => onplay(i)}
  />
{/snippet}

<div role="table" class="px-4" bind:this={host}>
  {#if !virtual}
    {#each indices as i (i)}
      {@render row(i)}
    {/each}
  {:else if layout.scrollEl}
    <Virtualizer data={indices} scrollRef={layout.scrollEl} itemSize={52} {startMargin} getKey={(i) => i}>
      {#snippet children(i: number)}
        {@render row(i)}
      {/snippet}
    </Virtualizer>
  {/if}
</div>
