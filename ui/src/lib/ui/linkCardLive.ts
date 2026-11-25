// The link card wired to the real core, player and toasts. Kept apart from the class so the
// class's tests don't load the player store.

import { invoke } from "$lib/ipc";
import { t } from "$lib/i18n";
import { player } from "$lib/stores/player.svelte";
import { toasts } from "$lib/stores/overlay.svelte";
import type { Track } from "$lib/ipc/bindings/Track";
import { LinkCardState } from "./linkCardState.svelte";

export function liveLinkCard() {
  return new LinkCardState({
    loadTrack: (id) => invoke<Track>("link_track", { id }),
    play: (id) => player.play({ kind: "tracks", trackIds: [id] }),
    queueAdd: (id, position) => player.queueAdd([id], position),
    done: (action) => {
      if (action === "next") toasts.push(t.trackMenu.playsNext);
      else if (action === "end") toasts.push(t.trackMenu.queued);
    },
    failed: (e) =>
      toasts.push(e instanceof Error && e.message ? e.message : t.link.openFailed, "error"),
  });
}
