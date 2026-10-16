<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import { t } from "$lib/i18n/ru";
  import IconButton from "./IconButton.svelte";
  import Slider from "./Slider.svelte";
  import IconVolume from "~icons/lucide/volume-2";
  import IconVolumeLow from "~icons/lucide/volume-1";
  import IconMute from "~icons/lucide/volume-x";

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

<div class="flex w-36 items-center gap-1">
  <IconButton label={t.player.mute} size="sm" onclick={toggleMute}>
    {#if player.state.volume === 0}<IconMute />{:else if player.state.volume < 0.5}<IconVolumeLow />{:else}<IconVolume />{/if}
  </IconButton>
  <Slider class="flex-1" label={t.player.volume} value={player.state.volume} max={1} oninput={(v) => player.setVolume(v)} onchange={(v) => player.setVolume(v)} />
</div>
