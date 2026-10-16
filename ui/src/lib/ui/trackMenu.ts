// Context menu for a track (US1, scenario 3).

import type { Track } from "$lib/ipc/bindings/Track";
import { invoke } from "$lib/ipc";
import { t } from "$lib/i18n/ru";
import { player } from "$lib/stores/player.svelte";
import { router } from "$lib/stores/router.svelte";
import { toasts, type MenuItem } from "$lib/stores/overlay.svelte";
import { playlistPicker } from "$lib/stores/playlistPicker.svelte";
import IconMix from "~icons/lucide/radio";
import IconNext from "~icons/lucide/list-start";
import IconQueue from "~icons/lucide/list-end";
import IconPlaylist from "~icons/lucide/list-plus";
import IconHeart from "~icons/lucide/heart";
import IconHeartOff from "~icons/lucide/heart-off";
import IconAlbum from "~icons/lucide/disc-3";
import IconArtist from "~icons/lucide/mic-vocal";
import IconDownload from "~icons/lucide/arrow-down-to-line";
import IconTrash from "~icons/lucide/trash-2";

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
      : { label: t.track.download, icon: IconDownload, disabled: true, action: () => {} },
  ];
}
