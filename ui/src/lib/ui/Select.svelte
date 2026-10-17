<script lang="ts" generics="T extends string">
  // Dropdown in the app's own menu style (native <select> popups look like Windows 95 in
  // WebView2). The chosen option has a check mark.
  import { menu } from "$lib/stores/overlay.svelte";
  import IconDown from "~icons/solar/alt-arrow-down-linear";
  import type { Component } from "svelte";

  interface Props {
    value: T;
    /** `icon` is shown next to the label, in the list and on the button. */
    options: { id: T; label: string; icon?: Component }[];
    label: string;
    onchange?: (value: T) => void;
    class?: string;
  }
  let { value = $bindable(), options, label, onchange, class: cls = "" }: Props = $props();
  let open = $state(false);
  const chosen = $derived(options.find((o) => o.id === value));
  const current = $derived(chosen?.label ?? "");

  function show(e: MouseEvent) {
    const el = e.currentTarget as HTMLElement;
    open = true;
    menu.showUnder(
      options.map((o) => ({
        label: o.label,
        icon: o.icon,
        checked: o.id === value,
        action: () => {
          value = o.id;
          onchange?.(o.id);
        },
      })),
      el,
    );
  }

  $effect(() => {
    if (!menu.open) open = false;
  });
</script>

<button
  type="button"
  aria-label={label}
  aria-haspopup="menu"
  aria-expanded={open}
  class="flex h-8 cursor-pointer items-center gap-1.5 rounded-md bg-fill-1 pr-2 pl-2.5 text-[12.5px] text-ink-2 ring-1 ring-line transition-colors hover:bg-fill-2 hover:text-ink
    {open ? 'bg-fill-2 text-ink' : ''} {cls}"
  onclick={show}
>
  {#if chosen?.icon}<chosen.icon class="size-4 shrink-0" />{/if}
  {current}
  <IconDown class="size-3.5 transition-transform duration-200 {open ? 'rotate-180' : ''}" />
</button>
