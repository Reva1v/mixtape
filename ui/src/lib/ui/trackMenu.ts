// Context menu for a track (US1, scenario 3).

import type { Track } from "$lib/ipc/bindings/Track";
import { invoke } from "$lib/ipc";
import { t } from "$lib/i18n/ru";
import { player } from "$lib/stores/player.svelte";
import { router } from "$lib/stores/router.svelte";
import { toasts, type MenuItem } from "$lib/stores/overlay.svelte";
import { playlistPicker } from "$lib/stores/playlistPicker.svelte";
import IconMix from "~icons/solar/radio-minimalistic-linear";
import IconNext from "~icons/solar/list-arrow-up-linear";
import IconQueue from "~icons/solar/list-arrow-down-linear";
import IconPlaylist from "~icons/solar/playlist-linear";
import IconHeart from "~icons/solar/heart-linear";
import IconHeartOff from "~icons/solar/heart-unlock-linear";
import IconAlbum from "~icons/solar/vinyl-linear";
import IconArtist from "~icons/solar/microphone-3-linear";
import IconDownload from "~icons/solar/download-minimalistic-linear";
import IconTrash from "~icons/solar/trash-bin-minimalistic-linear";

export function setLiked(track: Track, liked: boolean) {
  return invoke("track_set_liked", { id: track.id, liked });
}

export function trackMenu(track: Track, extra: MenuItem[] = []): MenuItem[] {
  const artist = track.artists.find((a) => a.id);
  const owned = track.state === "downloaded" || track.state === "broken";
  return [
    { label: t.track.mix, icon: IconMix, action: () => void player.mix(track.id) },
    "separator",
    {
      label: t.track.playNext,
      icon: IconNext,
      action: () => void player.queueAdd([track.id], "next").then(() => toasts.push("Сыграет следующим")),
    },
    {
      label: t.track.addToQueue,
      icon: IconQueue,
      action: () => void player.queueAdd([track.id], "end").then(() => toasts.push("Добавлено в очередь")),
    },
    { label: t.track.addToPlaylist, icon: IconPlaylist, action: () => playlistPicker.open([track.id]) },
    track.liked
      ? { label: t.track.unlike, icon: IconHeartOff, action: () => void setLiked(track, false) }
      : { label: t.track.like, icon: IconHeart, action: () => void setLiked(track, true) },
    "separator",
    {
      label: t.track.openAlbum,
      icon: IconAlbum,
      disabled: !track.album,
      action: () => track.album && router.go({ name: "album", id: track.album.id }),
    },
    {
      label: t.track.openArtist,
      icon: IconArtist,
      disabled: !artist,
      action: () => artist?.id && router.go({ name: "artist", id: artist.id }),
    },
    ...extra,
    "separator",
    owned
      ? { label: t.track.remove, icon: IconTrash, danger: true, disabled: true, action: () => {} }
      : {
          label: t.track.download,
          icon: IconDownload,
          action: () => void invoke("track_download", { id: track.id }).then(() => toasts.push("Добавлено в загрузки")),
        },
  ];
}
