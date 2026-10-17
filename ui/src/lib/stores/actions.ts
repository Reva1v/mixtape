// Shared click behaviour for album cards and tiles.

import type { Album } from "$lib/ipc/bindings/Album";
import { player } from "./player.svelte";
import { router } from "./router.svelte";

/** A one-track single is really a track: clicking it plays rather than opens a page. */
export function isSingleTrack(a: Album): boolean {
  return a.albumType === "single" && (a.trackCount ?? 1) <= 1;
}

export function isPlayingAlbum(a: Album): boolean {
  return player.track?.album?.id === a.id;
}

/** Plays (or pauses/resumes) a single; opens any other album. */
export function openAlbum(a: Album) {
  if (!isSingleTrack(a)) {
    router.go({ name: "album", id: a.id });
  } else if (isPlayingAlbum(a)) {
    void player.toggle();
  } else {
    void player.play({ kind: "album", id: a.id });
  }
}
