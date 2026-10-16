<script lang="ts">
  import { router } from "$lib/stores/router.svelte";
  import { t } from "$lib/i18n/ru";
  import IconButton from "./IconButton.svelte";
  import IconBack from "~icons/lucide/chevron-left";
  import IconForward from "~icons/lucide/chevron-right";
  import IconSearch from "~icons/lucide/search";

  let input = $state<HTMLInputElement>();
  let query = $state("");

  $effect(() => {
    if (router.current.name !== "search") query = "";
  });

  function onInput() {
    const route = { name: "search", query } as const;
    if (router.current.name === "search") router.replace(route);
    else router.go(route);
  }

  export function focusSearch() {
    input?.focus();
    input?.select();
  }
</script>

<header
  class="relative z-10 flex h-(--titlebar-h) shrink-0 items-center gap-1 pl-[calc(var(--sidebar-w)+8px)]"
  style:padding-right="calc(var(--tauri-frame-controls-width, 138px) + 8px)"
  data-tauri-drag-region
>
  <IconButton label={t.nav.back} size="sm" disabled={!router.canBack} onclick={() => router.back()}>
    <IconBack />
  </IconButton>
  <IconButton label={t.nav.forward} size="sm" disabled={!router.canForward} onclick={() => router.forward()}>
    <IconForward />
  </IconButton>

  <div class="pointer-events-none absolute inset-x-0 flex justify-center" data-tauri-drag-region>
    <label
      class="pointer-events-auto flex h-8 w-[min(440px,40vw)] items-center gap-2 rounded-full bg-fill-1 px-3 text-ink-3
        ring-1 ring-line transition-colors focus-within:bg-fill-2 focus-within:text-ink-2 hover:bg-fill-2"
    >
      <IconSearch class="size-4 shrink-0" />
      <input
        bind:this={input}
        bind:value={query}
        oninput={onInput}
        placeholder={t.search.placeholder}
        spellcheck="false"
        class="min-w-0 flex-1 bg-transparent text-[13px] text-ink outline-none placeholder:text-ink-3"
      />
      <kbd class="font-sans text-[11px] text-ink-3">{t.search.hint}</kbd>
    </label>
  </div>
</header>
