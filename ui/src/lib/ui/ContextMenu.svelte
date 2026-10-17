<script lang="ts">
  import { menu } from "$lib/stores/overlay.svelte";
  import { rise } from "$lib/motion";
  import IconCheck from "~icons/solar/unread-linear";

  let el = $state<HTMLDivElement>();
  let pos = $state({ x: 0, y: 0 });

  // Keep the menu inside the window.
  $effect(() => {
    if (!menu.open || !el) return;
    const r = el.getBoundingClientRect();
    pos = {
      x: Math.min(menu.x, window.innerWidth - r.width - 8),
      y: menu.y + r.height > window.innerHeight - 8 ? Math.max(8, menu.y - r.height) : menu.y,
    };
  });

  function onKey(e: KeyboardEvent) {
    if (menu.open && e.key === "Escape") {
      e.stopPropagation();
      menu.close();
    }
  }
</script>

<svelte:window onkeydown={onKey} onblur={() => menu.close()} />

{#if menu.open}
  <div
    class="fixed inset-0 z-50"
    role="presentation"
    onpointerdown={() => menu.close()}
    oncontextmenu={(e) => {
      e.preventDefault();
      menu.close();
    }}
  ></div>
  <!-- Opaque and unscaled on purpose: a fading backdrop-filter or a scale animation
       makes WebView2 draw the menu soft until the transition ends. -->
  <div
    bind:this={el}
    role="menu"
    tabindex="-1"
    class="fixed z-50 min-w-56 rounded-lg bg-raised p-1.5 shadow-pop"
    style:left="{pos.x}px"
    style:top="{pos.y}px"
    transition:rise={{ y: -6 }}
  >
    {#each menu.items as item, i (i)}
      {#if item === "separator"}
        <div class="mx-2 my-1 h-px bg-line"></div>
      {:else}
        <button
          type="button"
          role="menuitem"
          disabled={item.disabled}
          class="flex w-full items-center gap-3 rounded-md px-2.5 py-2 text-left text-[13px] transition-colors
            hover:bg-fill-2 disabled:pointer-events-none disabled:opacity-40
            {item.danger ? 'text-danger' : 'text-ink'}"
          onclick={() => {
            menu.close();
            item.action();
          }}
        >
          {#if item.icon}
            <item.icon class="size-4 shrink-0 text-ink-2" />
          {/if}
          <span class="flex-1">{item.label}</span>
          {#if item.checked}
            <IconCheck class="size-4 shrink-0 text-ink" />
          {/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}
