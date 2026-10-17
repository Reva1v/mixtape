// Spotify import (src-tauri/src/commands/spotify.rs): a public playlist or album link, or a
// pasted «Artist — Title» list, becomes a local playlist of YouTube Music songs.

import { invoke } from "./index";
import type { PlaylistId } from "./bindings/PlaylistId";
import type { SpotifyImportResult } from "./bindings/SpotifyImportResult";
import type { SpotifySource } from "./bindings/SpotifySource";

export type { SpotifyImportResult, SpotifySource };
export type { SpotifyProgress } from "./bindings/SpotifyProgress";
export type { SourceTrack } from "./bindings/SourceTrack";

export type SpotifyKind = "playlist" | "album";

/** Resolves with the summary once every track was looked up; rejects when cancelled. */
export const spotifyImport = (source: SpotifySource) => invoke<SpotifyImportResult>("spotify_import", { source });
export const spotifyCancel = () => invoke<void>("spotify_cancel");
/** Queues the playlist's tracks; resolves with how many were queued or moved from the cache. */
export const spotifyDownload = (id: PlaylistId) => invoke<number>("spotify_download", { id });

/** Recognises `open.spotify.com/{playlist,album}/<id>` (with `intl-xx/`, `embed/`, a query
 * or no scheme) and `spotify:playlist:<id>`; mirrors `mixtape_sync::spotify::parse_link`. */
export function parseSpotifyUrl(text: string): { url: string; kind: SpotifyKind } | null {
  const s = text.trim();
  const uri = /^spotify:(?:user:[^:]+:)?(playlist|album):([A-Za-z0-9]{22})$/.exec(s);
  if (uri) return { url: s, kind: uri[1] as SpotifyKind };
  let u: URL;
  try {
    u = new URL(/^[a-z]+:\/\//i.test(s) ? s : `https://${s}`);
  } catch {
    return null;
  }
  if (u.protocol !== "https:" && u.protocol !== "http:") return null;
  if (u.hostname !== "open.spotify.com" && u.hostname !== "play.spotify.com") return null;
  const segs = u.pathname.split("/").filter(Boolean);
  if (segs[0]?.startsWith("intl-")) segs.shift();
  if (segs[0] === "embed") segs.shift();
  if (segs[0] === "user" && segs.length >= 4) segs.splice(0, 2);
  if (segs.length !== 2 || !/^[A-Za-z0-9]{22}$/.test(segs[1])) return null;
  if (segs[0] !== "playlist" && segs[0] !== "album") return null;
  return { url: u.href, kind: segs[0] };
}

/** «Artist, Artist — Title» per line, as the not-found list is copied. */
export function trackLine(t: { title: string; artists: string[] }): string {
  return t.artists.length ? `${t.artists.join(", ")} — ${t.title}` : t.title;
}
