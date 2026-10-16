<script lang="ts">
  // Virtualized track list that scrolls with the page (scrollRef = main area), so
  // headers above it scroll away naturally.
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

  const indices = $derived(Array.from({ length: rows.count }, (_, i) => i));
</script>

{#if layout.scrollEl}
  <div role="table" class="px-4">
    <Virtualizer data={indices} scrollRef={layout.scrollEl} itemSize={52} getKey={(i) => i}>
      {#snippet children(i: number)}
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
    </Virtualizer>
  </div>
{/if}
