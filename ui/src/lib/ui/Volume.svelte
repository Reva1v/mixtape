<script lang="ts">
  // Volume: an icon that mutes; the slider slides out on hover in compact mode.
  import { player } from "$lib/stores/player.svelte";
  import { t } from "$lib/i18n/ru";
  import Slider from "./Slider.svelte";
  import { onWheel } from "$lib/motion";
  import IconVolume from "~icons/solar/volume-loud-linear";
  import IconVolumeLow from "~icons/solar/volume-small-linear";
  import IconMute from "~icons/solar/volume-cross-linear";

  let { class: cls = "w-36", compact = false }: { class?: string; compact?: boolean } = $props();

  let restore = 0.8;

  // Compact mode opens on hover intent, not on a passing cursor: the capsule is anchored
  // right, so an instant open grows it leftwards under the pointer heading elsewhere.
  const OPEN_DELAY = 300;
  const CLOSE_DELAY = 350;
  let open = $state(false);
  let timer = 0;
  function hover(inside: boolean) {
    clearTimeout(timer);
    if (inside === open) return;
    timer = window.setTimeout(() => (open = inside), inside ? OPEN_DELAY : CLOSE_DELAY);
  }
  $effect(() => () => clearTimeout(timer));

  // The wheel works over the icon too, not only over the slider.
  function wheel(e: WheelEvent) {
    if (e.deltaY === 0) return;
    e.preventDefault();
    e.stopPropagation();
    const v = Math.min(1, Math.max(0, player.state.volume + (e.deltaY < 0 ? 0.05 : -0.05)));
    void player.setVolume(Math.round(v * 100) / 100);
  }

  function toggleMute() {
    if (player.state.volume > 0) {
      restore = player.state.volume;
      void player.setVolume(0);
    } else {
      void player.setVolume(restore || 0.8);
    }
  }
</script>

<div
  class="group/vol flex items-center {compact ? '' : `gap-2 ${cls}`}"
  {@attach onWheel(wheel)}
  role="group"
  aria-label={t.player.volume}
  onpointerenter={() => compact && hover(true)}
  onpointerleave={() => compact && hover(false)}
>
  <button
    type="button"
    aria-label={t.player.mute}
    data-tip={t.player.volume}
    class="grid size-8 shrink-0 place-items-center rounded-lg text-ink-2 transition-colors hover:text-ink"
    onclick={toggleMute}
  >
    {#if player.state.volume === 0}<IconMute class="size-[18px]" />{:else if player.state.volume < 0.5}<IconVolumeLow class="size-[18px]" />{:else}<IconVolume class="size-[18px]" />{/if}
  </button>
  <div
    class={compact
      ? `overflow-hidden transition-all duration-200 ease-out-soft group-has-[:focus-visible]/vol:w-24 group-has-[:focus-visible]/vol:opacity-100 ${open ? "w-24 pr-2 opacity-100" : "w-0 opacity-0"}`
      : "flex-1"}
  >
    <Slider
      class={compact ? "w-24" : "w-full"}
      label={t.player.volume}
      value={player.state.volume}
      max={1}
      oninput={(v) => player.setVolume(v)}
      onchange={(v) => player.setVolume(v)}
    />
  </div>
</div>
