<script lang="ts">
  // The "?" dialog: every keyboard shortcut, grouped.
  import { layout } from "$lib/stores/layout.svelte";
  import { shortcuts } from "$lib/stores/hotkeys";
  import Modal from "./Modal.svelte";
</script>

<Modal open={layout.shortcutsOpen} title="Горячие клавиши" onclose={() => (layout.shortcutsOpen = false)} width={560}>
  <div class="space-y-6">
    {#each shortcuts as g (g.group)}
      <section>
        <h3 class="mb-2 text-[12px] font-semibold tracking-wide text-ink-3 uppercase">{g.group}</h3>
        <div class="divide-y divide-line">
          {#each g.items as [keys, action] (keys)}
            <div class="flex items-center justify-between gap-4 py-2">
              <span class="text-[13.5px] text-ink-2">{action}</span>
              <span class="flex shrink-0 flex-wrap justify-end gap-1">
                {#each keys.split("  ·  ") as alt, i (i)}
                  {#if i > 0}<span class="px-0.5 text-[12px] text-ink-3">или</span>{/if}
                  <kbd class="rounded-md bg-fill-2 px-2 py-0.5 font-sans text-[12px] text-ink ring-1 ring-line">{alt}</kbd>
                {/each}
              </span>
            </div>
          {/each}
        </div>
      </section>
    {/each}
  </div>
</Modal>
