// «Поделиться»: puts the plain music.youtube.com link of a track, album, playlist or artist
// on the clipboard. The URL rules live in Rust (`link_share_url`); this is only the glue.
// Never a `mixtape://` link: whoever receives it may not have Mixtape (owner, 2026-11-25).

import { invoke } from "$lib/ipc";
import { t } from "$lib/i18n";
import { toasts } from "$lib/stores/overlay.svelte";
import type { SharedKind } from "$lib/ipc/bindings/SharedKind";

/** A YouTube Music `videoId`: exactly 11 characters. Local files (`local0000000001`),
 * SoundCloud and Bandcamp ids (`sc` + 16 hex, `bc…`) are longer, so there's no link to share. */
export const isYtmTrack = (id: string) => /^[A-Za-z0-9_-]{11}$/.test(id);

/** `VL…` / `PL…` are YouTube Music playlists; `local:<uuid>` lives only on this computer. */
export const isYtmPlaylist = (id: string) => !id.startsWith("local:");

export async function share(kind: SharedKind, id: string) {
  try {
    const url = await invoke<string>("link_share_url", { kind, id });
    await navigator.clipboard.writeText(url);
    toasts.push(t.share.copied);
  } catch (e) {
    toasts.push(e instanceof Error && e.message ? e.message : t.share.copyFailed, "error");
  }
}
