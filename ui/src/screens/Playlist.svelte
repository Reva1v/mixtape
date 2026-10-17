<script lang="ts" module>
  import type { PlaylistPage as Cached } from "$lib/ipc/bindings/PlaylistPage";
  // Recently shown pages: going back renders at full height at once, so the scroll
  // position is restored without a jump; a fresh copy replaces it quietly.
  const cache = new Map<string, Cached>();
  function remember(id: string, page: Cached) {
    cache.delete(id);
    cache.set(id, page);
    if (cache.size > 20) cache.delete(cache.keys().next().value!);
  }
</script>

<script lang="ts">
  import { invoke, on } from "$lib/ipc";
  import type { PlaylistPage } from "$lib/ipc/bindings/PlaylistPage";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { formatTime, player } from "$lib/stores/player.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { tracks } from "$lib/i18n/ru";
  import { menu, prompt, toasts } from "$lib/stores/overlay.svelte";
  import CollectionHeader from "$lib/ui/CollectionHeader.svelte";
  import TrackList from "$lib/ui/TrackList.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import Skeleton from "$lib/ui/Skeleton.svelte";
  import IconButton from "$lib/ui/IconButton.svelte";
  import IconAlert from "~icons/solar/danger-circle-linear";
  import IconList from "~icons/solar/playlist-minimalistic-linear";
  import IconMore from "~icons/solar/menu-dots-bold";
  import IconTrash from "~icons/solar/trash-bin-minimalistic-linear";
  import IconPencil from "~icons/solar/pen-linear";

  let { id }: { id: string } = $props();

  // svelte-ignore state_referenced_locally
  let page = $state.raw<PlaylistPage | null>(cache.get(id) ?? null);
  let error = $state<string | null>(null);
  // svelte-ignore state_referenced_locally
  const rows = new Fixed<Track>(page?.tracks ?? []);

  async function load() {
    try {
      page = await invoke<PlaylistPage>("playlist_get", { id });
      remember(id, page);
      rows.items = page.tracks;
    } catch (e) {
      error = (e as Error).message;
    }
  }

  $effect(() => {
    void load();
    const un = on("library:changed", () => void load());
    return () => void un.then((f) => f());
  });

  const total = $derived(page ? page.tracks.reduce((s, t) => s + t.durationMs, 0) : 0);

  function play(i = 0, shuffle = false) {
    if (shuffle !== player.state.shuffle) void player.toggleShuffle();
    void player.play({ kind: "playlist", id }, i);
  }

  async function remove() {
    await invoke("playlist_edit", { edit: { op: "delete", id } });
    toasts.push("Плейлист удалён");
    router.back();
  }

  async function rename() {
    const title = await prompt.ask("Название плейлиста", page?.playlist.title ?? "");
    if (!title) return;
    const newId = await invoke<string | null>("playlist_edit", { edit: { op: "rename", id, title } });
    if (newId && newId !== id) router.replace({ name: "playlist", id: newId });
  }
  import DownloadButton from "$lib/ui/DownloadButton.svelte";
</script>

{#if error}
  <EmptyState icon={IconAlert} title="Не удалось открыть плейлист" hint={error} />
{:else if !page}
  <div class="flex items-end gap-7 px-7 pt-10">
    <Skeleton class="size-52 rounded-lg" />
    <div class="flex-1 space-y-3"><Skeleton class="h-10 w-1/2" /><Skeleton class="h-4 w-1/3" /></div>
  </div>
{:else}
  <CollectionHeader
    kind={page.playlist.origin === "ytm" ? "Плейлист YouTube Music" : "Плейлист"}
    title={page.playlist.title}
    coverKey={page.playlist.coverKey}
    onplay={page.tracks.length ? () => play(0) : undefined}
    current={page.tracks.some((x) => x.id === player.currentId)}
    onshuffle={page.tracks.length ? () => play(0, true) : undefined}
  >
    {#snippet meta()}
      <span>{tracks(page!.tracks.length)}{total ? `, ${formatTime(total)}` : ""}</span>
    {/snippet}
    {#snippet actions()}
      {#if page!.playlist.origin === "ytm" && page!.tracks.some((t) => t.state === "remote" || t.state === "cached")}
        <DownloadButton kind="playlist" id={page!.playlist.id} label="Скачать плейлист" />
      {/if}
      <IconButton
        label="Ещё"
        onclick={(e) =>
          menu.showAt(
            [
              { label: "Переименовать", icon: IconPencil, action: rename },
              "separator",
              { label: "Удалить плейлист", icon: IconTrash, danger: true, action: remove },
            ],
            e.currentTarget,
          )}
      >
        <IconMore />
      </IconButton>
    {/snippet}
  </CollectionHeader>
  <div class="pb-10">
    {#if page.tracks.length === 0}
      <EmptyState icon={IconList} title="Плейлист пуст" hint="Добавляйте треки через меню трека" />
    {:else}
      <TrackList {rows} onplay={(i) => play(i)} />
    {/if}
  </div>
{/if}
