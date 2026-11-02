<script lang="ts">
  import { toasts } from "$lib/stores/overlay.svelte";
  import { pop } from "$lib/motion";
  import { flip } from "svelte/animate";
</script>

<!-- `w-max`: a box centred with left-1/2 is otherwise only half the window wide, and wraps. -->
<div
  class="pointer-events-none fixed bottom-[calc(var(--player-h)+16px)] left-1/2 z-60 flex w-max max-w-[calc(100vw-32px)] -translate-x-1/2 flex-col items-center gap-2"
>
  {#each toasts.list as toast (toast.id)}
    {@const tone = toast.level === "error" ? "bg-danger text-white" : "bg-raised text-ink"}
    <div
      animate:flip={{ duration: 260, easing: (t) => 1 - (1 - t) ** 3 }}
      transition:pop={{ y: 14, scale: 0.9 }}
      class="pointer-events-auto flex max-w-full items-center rounded-[18px] text-[13px] shadow-pop {tone}"
    >
      <button
        type="button"
        class="min-w-0 py-2 pl-4 text-left {toast.action ? 'pr-3' : 'pr-4'}"
        onclick={() => toasts.dismiss(toast.id)}
      >
        {toast.text}
      </button>
      {#if toast.action}
        <button
          type="button"
          class="mr-1 h-7 shrink-0 whitespace-nowrap rounded-full px-3 font-semibold transition-colors
            {toast.level === 'error'
            ? 'bg-white/20 hover:bg-white/30'
            : 'bg-ink text-black hover:bg-[#dcdcdc]'}"
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
