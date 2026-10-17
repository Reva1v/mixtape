// Context menus for album, artist and playlist cards, in the same shape as the track
// menu. Track lists are fetched only when an action needs them.

import type { Album } from "$lib/ipc/bindings/Album";
import type { AlbumPage } from "$lib/ipc/bindings/AlbumPage";
import type { Artist } from "$lib/ipc/bindings/Artist";
import type { ArtistPage } from "$lib/ipc/bindings/ArtistPage";
import type { Playlist } from "$lib/ipc/bindings/Playlist";
import type { PlaylistPage } from "$lib/ipc/bindings/PlaylistPage";
import type { PlaySource } from "$lib/ipc/bindings/PlaySource";
import type { Track } from "$lib/ipc/bindings/Track";
import { invoke } from "$lib/ipc";
import { t } from "$lib/i18n";
import { player } from "$lib/stores/player.svelte";
import { router } from "$lib/stores/router.svelte";
import { toasts, type MenuItem } from "$lib/stores/overlay.svelte";
import { playlistPicker } from "$lib/stores/playlistPicker.svelte";
import IconMix from "~icons/solar/radio-minimalistic-linear";
import IconPlay from "~icons/solar/play-linear";
import IconShuffle from "~icons/solar/shuffle-linear";
import IconNext from "~icons/solar/list-arrow-up-linear";
import IconQueue from "~icons/solar/list-arrow-down-linear";
import IconPlaylist from "~icons/solar/playlist-linear";
import IconAlbum from "~icons/solar/vinyl-linear";
import IconArtist from "~icons/solar/microphone-3-linear";

type Load = () => Promise<Track[]>;

const ids = (tracks: Track[]) => tracks.map((x) => x.id);

function failed(e: unknown) {
  toasts.push(e instanceof Error && e.message ? e.message : t.trackMenu.failed, "error");
}

/** Play, shuffle, queue and playlist items shared by every collection. */
function common(source: PlaySource, load: Load, mix = true): MenuItem[] {
  return [
    ...(mix
      ? ([
          {
            label: t.track.mix,
            icon: IconMix,
            action: () => void load().then((ts) => ts[0] && player.mix(ts[0].id), failed),
          },
          "separator",
        ] as MenuItem[])
      : []),
    { label: t.player.play, icon: IconPlay, action: () => void player.play(source) },
    {
      label: t.player.shuffle,
      icon: IconShuffle,
      action: () =>
        void load().then((ts) => ts.length && player.play({ kind: "tracks", trackIds: shuffled(ids(ts)) }), failed),
    },
    "separator",
    {
      label: t.track.playNext,
      icon: IconNext,
      action: () => void load().then((ts) => player.queueAdd(ids(ts), "next")).then(() => toasts.push(t.trackMenu.playsNext), failed),
    },
    {
      label: t.track.addToQueue,
      icon: IconQueue,
      action: () => void load().then((ts) => player.queueAdd(ids(ts), "end")).then(() => toasts.push(t.trackMenu.queued), failed),
    },
    {
      label: t.track.addToPlaylist,
      icon: IconPlaylist,
      action: () => void load().then((ts) => playlistPicker.open(ids(ts)), failed),
    },
  ];
}

function shuffled<T>(xs: T[]): T[] {
  const out = [...xs];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

export function albumMenu(a: Album): MenuItem[] {
  const load = () => invoke<AlbumPage>("album_get", { id: a.id }).then((p) => p.tracks);
  const artist = a.artists.find((x) => x.id);
  return [
    ...common({ kind: "album", id: a.id }, load),
    "separator",
    { label: t.track.openAlbum, icon: IconAlbum, action: () => router.go({ name: "album", id: a.id }) },
    {
      label: t.track.openArtist,
      icon: IconArtist,
      disabled: !artist,
      action: () => artist?.id && router.go({ name: "artist", id: artist.id }),
    },
  ];
}

export function artistMenu(a: Artist): MenuItem[] {
  const load = () => invoke<ArtistPage>("artist_get", { id: a.id }).then((p) => p.tracks);
  return [
    ...common({ kind: "artist", id: a.id }, load),
    "separator",
    { label: t.track.openArtist, icon: IconArtist, action: () => router.go({ name: "artist", id: a.id }) },
  ];
}

export function playlistMenu(p: Playlist): MenuItem[] {
  const load = () => invoke<PlaylistPage>("playlist_get", { id: p.id }).then((x) => x.tracks);
  return [
    ...common({ kind: "playlist", id: p.id }, load, false),
    "separator",
    { label: t.collectionMenu.openPlaylist, icon: IconPlaylist, action: () => router.go({ name: "playlist", id: p.id }) },
  ];
}
