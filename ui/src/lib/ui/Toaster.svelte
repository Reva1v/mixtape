<script lang="ts">
  import { toasts } from "$lib/stores/overlay.svelte";
  import { pop } from "$lib/motion";
  import { flip } from "svelte/animate";
</script>

<div class="pointer-events-none fixed bottom-[calc(var(--player-h)+16px)] left-1/2 z-60 flex -translate-x-1/2 flex-col items-center gap-2">
  {#each toasts.list as toast (toast.id)}
    {@const tone = toast.level === "error" ? "bg-danger/90 text-white" : "bg-raised/95 text-ink"}
    <div
      animate:flip={{ duration: 260, easing: (t) => 1 - (1 - t) ** 3 }}
      transition:pop={{ y: 14, scale: 0.9 }}
      class="pointer-events-auto flex items-center rounded-full text-[13px] shadow-pop backdrop-blur-xl {tone}"
    >
      <button type="button" class="py-2 pl-4 {toast.action ? 'pr-2' : 'pr-4'}" onclick={() => toasts.dismiss(toast.id)}>
        {toast.text}
      </button>
      {#if toast.action}
        <button
          type="button"
          class="mr-1 h-7 rounded-full px-3 font-semibold transition-colors
            {toast.level === 'error' ? 'bg-white/20 hover:bg-white/30' : 'bg-ink text-black hover:bg-[#dcdcdc]'}"
          onclick={() => {
            toasts.dismiss(toast.id);
            toast.action?.run();
          }}
        >
          {toast.action.label}
        </button>
      {/if}
    </div>
  {/each}
</div>
