<script lang="ts">
  // Artist names as links to their pages; names without an id stay plain text.
  import type { ArtistRef } from "$lib/ipc/bindings/ArtistRef";
  import { router } from "$lib/stores/router.svelte";

  let { artists }: { artists: ArtistRef[] } = $props();
</script>

{#each artists as a, i (i)}
  {#if i > 0},&nbsp;{/if}
  {#if a.id}
    <button
      type="button"
      class="hover:text-ink hover:underline"
      onclick={(e) => {
        e.stopPropagation();
        router.go({ name: "artist", id: a.id! });
      }}>{a.name}</button
    >
  {:else}{a.name}{/if}
{/each}
