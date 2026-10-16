<script lang="ts">
  // App-wide dialogs: text prompt and "add to playlist" picker.
  import { invoke } from "$lib/ipc";
  import type { ViewInfo } from "$lib/ipc/bindings/ViewInfo";
  import type { Rows } from "$lib/ipc/bindings/Rows";
  import type { Playlist } from "$lib/ipc/bindings/Playlist";
  import { prompt, toasts } from "$lib/stores/overlay.svelte";
  import { playlistPicker } from "$lib/stores/playlistPicker.svelte";
  import { t } from "$lib/i18n/ru";
  import Modal from "./Modal.svelte";
  import Cover from "./Cover.svelte";
  import IconPlus from "~icons/lucide/plus";

  let input = $state<HTMLInputElement>();
  $effect(() => {
    if (prompt.open) queueMicrotask(() => input?.select());
  });

  let playlists = $state.raw<Playlist[]>([]);
  $effect(() => {
    if (!playlistPicker.trackIds) return;
    void (async () => {
      const v = await invoke<ViewInfo>("library_view", { kind: "playlists", sort: "recent" });
      const rows = await invoke<Rows>("library_rows", { viewId: v.viewId, offset: 0, limit: 200 });
      playlists = (rows.items as Playlist[]).filter((p) => p.origin === "local");
    })();
  });

  async function addTo(id: string, title: string) {
    const trackIds = playlistPicker.trackIds;
    playlistPicker.close();
    if (!trackIds) return;
    await invoke("playlist_edit", { edit: { op: "add", id, trackIds } });
    toasts.push(`Добавлено в «${title}»`);
  }

  async function createAndAdd() {
    const trackIds = playlistPicker.trackIds;
    playlistPicker.close();
    const title = await prompt.ask("Новый плейлист");
    if (!title || !trackIds) return;
    const id = await invoke<string>("playlist_edit", { edit: { op: "create", title } });
    await invoke("playlist_edit", { edit: { op: "add", id, trackIds } });
    toasts.push(`Добавлено в «${title}»`);
  }
</script>

<Modal open={prompt.open} title={prompt.title} onclose={() => prompt.finish(false)} width={400}>
  <form
    onsubmit={(e) => {
      e.preventDefault();
      prompt.finish(true);
    }}
  >
    <input
      bind:this={input}
      bind:value={prompt.value}
      class="h-10 w-full rounded-md bg-fill-2 px-3 text-[14px] outline-none ring-1 ring-line focus:ring-(--accent)"
    />
    <div class="mt-5 flex justify-end gap-2">
      <button type="button" class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2" onclick={() => prompt.finish(false)}>
        {t.common.cancel}
      </button>
      <button type="submit" class="h-9 rounded-full bg-(--accent) px-5 text-[13px] font-semibold text-white">{t.common.ok}</button>
    </div>
  </form>
</Modal>

<Modal open={!!playlistPicker.trackIds} title={t.track.addToPlaylist} onclose={() => playlistPicker.close()} width={380}>
  <div class="-mx-2 max-h-80 overflow-y-auto">
    <button type="button" class="flex w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-fill-2" onclick={createAndAdd}>
      <div class="grid size-10 place-items-center rounded-sm bg-fill-2"><IconPlus class="size-5" /></div>
      <span class="text-[14px] font-medium">Новый плейлист</span>
    </button>
    {#each playlists as p (p.id)}
      <button type="button" class="flex w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-fill-2" onclick={() => addTo(p.id, p.title)}>
        <Cover coverKey={p.coverKey} size={40} rounded="sm" />
        <div class="min-w-0">
          <div class="truncate text-[14px]">{p.title}</div>
          <div class="text-[12px] text-ink-3">{p.trackCount} треков</div>
        </div>
      </button>
    {/each}
  </div>
</Modal>
