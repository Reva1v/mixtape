<script lang="ts">
  // Volume: an icon that mutes; the slider slides out on hover in compact mode.
  import { player } from "$lib/stores/player.svelte";
  import { t } from "$lib/i18n/ru";
  import Slider from "./Slider.svelte";
  import IconVolume from "~icons/solar/volume-loud-linear";
  import IconVolumeLow from "~icons/solar/volume-small-linear";
  import IconMute from "~icons/solar/volume-cross-linear";

  let { class: cls = "w-36", compact = false }: { class?: string; compact?: boolean } = $props();

  let restore = 0.8;

  function toggleMute() {
    if (player.state.volume > 0) {
      restore = player.state.volume;
      void player.setVolume(0);
    } else {
      void player.setVolume(restore || 0.8);
    }
  }
</script>

<div class="group/vol flex items-center {compact ? '' : `gap-2 ${cls}`}">
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
      ? "w-0 overflow-hidden opacity-0 transition-all duration-200 ease-out-soft group-hover/vol:w-24 group-hover/vol:pr-2 group-hover/vol:opacity-100 group-focus-within/vol:w-24 group-focus-within/vol:opacity-100"
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
