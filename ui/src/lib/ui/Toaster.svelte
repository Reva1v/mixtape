<script lang="ts">
  import { toasts } from "$lib/stores/overlay.svelte";
  import { fly } from "svelte/transition";
  import { flip } from "svelte/animate";
</script>

<div class="pointer-events-none fixed bottom-[calc(var(--player-h)+16px)] left-1/2 z-60 flex -translate-x-1/2 flex-col items-center gap-2">
  {#each toasts.list as toast (toast.id)}
    <button
      type="button"
      animate:flip={{ duration: 200 }}
      transition:fly={{ y: 12, duration: 200 }}
      class="pointer-events-auto rounded-full px-4 py-2 text-[13px] shadow-pop backdrop-blur-xl
        {toast.level === 'error' ? 'bg-danger/90 text-white' : 'bg-[#23232c]/95 text-ink'}"
      onclick={() => toasts.dismiss(toast.id)}
    >
      {toast.text}
    </button>
  {/each}
</div>
