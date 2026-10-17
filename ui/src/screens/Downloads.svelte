<script lang="ts">
  // Sync and downloads (T066): what to fetch from YouTube Music, overall progress, the
  // track being downloaded, and downloads that failed with their reasons.
  import { syncStart } from "$lib/ipc/sync";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { pressable } from "$lib/motion";
  import { plural, t, tracks } from "$lib/i18n/ru";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import IconButton from "$lib/ui/IconButton.svelte";
  import Cover from "$lib/ui/Cover.svelte";
  import IconDownload from "~icons/solar/download-minimalistic-bold";
  import IconLink from "~icons/solar/link-linear";
  import IconPause from "~icons/solar/pause-bold";
  import IconPlay from "~icons/solar/play-bold";
  import IconAlert from "~icons/solar/danger-triangle-linear";
  import IconHeart from "~icons/solar/heart-bold";
  import IconAlbum from "~icons/solar/vinyl-linear";
  import IconArtist from "~icons/solar/microphone-3-linear";
  import IconCheck from "~icons/lucide/check";

  const account = $derived(downloads.account);
  const progress = $derived(downloads.progress);

  // The list and the picks live in the store: no reload and no jump on every visit.
  const preview = $derived(downloads.preview);
  const previewError = $derived(downloads.previewError);
  const pick = downloads.pick;
  let starting = $state(false);

  $effect(() => {
    if (account.connected) void downloads.loadPreview();
  });

  const allPlaylists = $derived(!!preview && preview.playlists.length > 0 && pick.playlistIds.length === preview.playlists.length);
  const nothingChosen = $derived(!pick.liked && !pick.albums && !pick.artists && pick.playlistIds.length === 0);

  function togglePlaylist(id: string, on: boolean) {
    pick.playlistIds = on ? [...pick.playlistIds, id] : pick.playlistIds.filter((x) => x !== id);
  }

  async function start() {
    if (nothingChosen || starting) return;
    starting = true;
    try {
      await syncStart({ ...pick, playlistIds: [...pick.playlistIds] });
      toasts.push("Синхронизация запущена — скачиваются только новые треки");
    } catch (e) {
      toasts.push(e instanceof Error && e.message ? e.message : "Не удалось запустить синхронизацию", "error");
    } finally {
      starting = false;
    }
  }

  function togglePause() {
    void (progress.paused ? downloads.resume() : downloads.pause()).catch((e: unknown) =>
      toasts.push(e instanceof Error ? e.message : String(e), "error"),
    );
  }

  function retry(jobId: number | "all-failed") {
    void downloads.retry(jobId).catch((e: unknown) => toasts.push(e instanceof Error ? e.message : String(e), "error"));
  }

  /** "3,4 МБ": megabytes with one decimal, Russian formatting. */
  const mb = (bytes: number) => (bytes / 1048576).toLocaleString("ru-RU", { maximumFractionDigits: 1 });

  const percent = $derived(progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : 0);
  const finished = $derived(!progress.running && progress.total > 0 && progress.done + progress.failed >= progress.total);
  const status = $derived(
    progress.paused ? "На паузе" : progress.running ? "Скачивается" : finished ? "Готово" : "Остановлено",
  );
  const showProgress = $derived(progress.running || progress.total > 0);
  const sync = $derived(progress.sync);
  const syncPercent = $derived(sync && sync.total > 0 ? Math.min(100, (sync.done / sync.total) * 100) : 0);
  const empty = $derived(!account.connected && !showProgress && downloads.failed.length === 0);
</script>

{#snippet checkbox(checked: boolean, label: string, onchange: (v: boolean) => void)}
  <span class="relative grid size-[18px] shrink-0 place-items-center">
    <input
      type="checkbox"
      {checked}
      aria-label={label}
      class="peer size-full cursor-pointer appearance-none rounded-[5px] ring-1 ring-white/25 transition-colors ring-inset
        checked:bg-ink checked:ring-ink hover:ring-white/45"
      onchange={(e) => onchange(e.currentTarget.checked)}
    />
    <IconCheck class="pointer-events-none absolute size-3 text-black opacity-0 peer-checked:opacity-100" stroke-width="3" />
  </span>
{/snippet}

{#snippet sourceRow(checked: boolean, title: string, sub: string, icon: typeof IconHeart, onchange: (v: boolean) => void)}
  {@const Icon = icon}
  <label class="flex h-[60px] items-center gap-4 px-5 transition-colors hover:bg-fill-1">
    {@render checkbox(checked, title, onchange)}
    <div class="grid size-[40px] shrink-0 place-items-center rounded-sm bg-fill-2 text-ink-2"><Icon class="size-[20px]" /></div>
    <div class="min-w-0 flex-1">
      <div class="truncate text-[14px]">{title}</div>
      <div class="truncate text-[12.5px] text-ink-3">{sub}</div>
    </div>
  </label>
{/snippet}

<div class="mx-auto flex min-h-full w-full max-w-[924px] flex-col px-8 pt-10 pb-12">
  <div class="mb-8 flex items-end justify-between gap-4">
    <h1 class="text-[28px] font-bold tracking-tight">{t.nav.downloads}</h1>
    <button
      type="button"
      class="flex h-9 items-center gap-2 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3"
      data-tip="Ctrl + L"
      onclick={() => (downloads.importUrl = "")}
    >
      <IconLink class="size-4" />Добавить по ссылке
    </button>
  </div>

  {#if empty}
    <!-- Centred in the panel; the negative margin offsets the header for optical balance. -->
    <div class="-mt-16 grid flex-1 place-items-center">
    <EmptyState
      icon={IconDownload}
      title="YouTube Music не подключён"
      hint="Подключите аккаунт, чтобы выкачать лайки, плейлисты, альбомы и исполнителей. Отдельный трек, альбом или плейлист можно добавить по ссылке."
    >
      <button
        type="button"
        class="h-9 rounded-full bg-ink px-5 text-[13px] font-semibold text-black transition-[background-color] hover:bg-[#dcdcdc]"
        {@attach pressable(0.95)}
        onclick={() => (downloads.connectOpen = true)}
      >
        Подключить
      </button>
    </EmptyState>
    </div>
  {:else}
    {#if sync}
      <section class="mb-4 rounded-lg bg-white/[0.04] px-5 py-4" aria-live="polite">
        <div class="flex items-baseline gap-2">
          <span class="text-[14.5px] font-medium">Синхронизация</span>
          <span class="text-[13px] text-ink-3 tnum">{sync.label}{#if sync.total > 0}&nbsp;· {sync.done} из {sync.total}{/if}…</span>
        </div>
        <div class="mt-0.5 text-[13px] text-ink-3">Читаю YouTube Music — загрузка начнётся сразу после этого</div>
        <div class="mt-3 h-[3px] overflow-hidden rounded-full bg-fill-2" role="progressbar" aria-label="Синхронизация">
          {#if sync.total > 0}
            <div class="h-full rounded-full bg-ink transition-[width] duration-(--spring-smooth-ms)" style:width="{syncPercent}%"></div>
          {:else}
            <div class="indeterminate h-full w-1/3 rounded-full bg-ink"></div>
          {/if}
        </div>
      </section>
    {/if}
    {#if showProgress}
      <section class="mb-8 rounded-lg bg-white/[0.04] px-5 py-4">
        <div class="flex items-center gap-4">
          <div class="min-w-0 flex-1">
            <div class="flex items-baseline gap-2">
              <span class="text-[14.5px] font-medium">{status}</span>
              <span class="text-[13px] text-ink-3 tnum">
                {progress.done} из {progress.total}{#if progress.failed > 0}&nbsp;· с ошибкой {progress.failed}{/if}
              </span>
            </div>
            <div class="mt-0.5 truncate text-[13px] text-ink-3 tnum">
              {#if progress.current && progress.running}
                <span class="text-ink-2">{progress.current.title}</span>
                &nbsp;·&nbsp;{mb(progress.current.bytesDone)}{#if progress.current.bytesTotal}&nbsp;из {mb(progress.current.bytesTotal)}{/if} МБ
                {#if !progress.paused && progress.bytesPerSec > 0}&nbsp;·&nbsp;{mb(progress.bytesPerSec)} МБ/с{/if}
              {:else if finished}
                Всё, что удалось, уже в библиотеке
              {:else}
                Осталось {tracks(Math.max(0, progress.total - progress.done - progress.failed))}
              {/if}
            </div>
          </div>
          {#if progress.running}
            <IconButton label={progress.paused ? "Продолжить" : "Пауза"} onclick={togglePause}>
              {#if progress.paused}<IconPlay class="size-[18px]" />{:else}<IconPause class="size-[18px]" />{/if}
            </IconButton>
          {/if}
        </div>
        <div class="mt-3 h-[3px] overflow-hidden rounded-full bg-fill-2" role="progressbar" aria-valuenow={Math.round(percent)} aria-valuemin={0} aria-valuemax={100}>
          <div
            class="h-full rounded-full transition-[width,background-color] duration-(--spring-smooth-ms) {progress.paused ? 'bg-ink-3' : 'bg-ink'}"
            style:width="{percent}%"
          ></div>
        </div>
      </section>
    {/if}

    {#if account.connected}
      <section class="mb-8">
        <div class="mb-3 flex items-center justify-between">
          <h2 class="text-[13px] font-semibold tracking-wide text-ink-3 uppercase">Что скачать</h2>
          {#if preview && preview.playlists.length > 0}
            <button
              type="button"
              class="text-[12.5px] text-ink-3 transition-colors hover:text-ink"
              onclick={() => (pick.playlistIds = allPlaylists ? [] : preview!.playlists.map((p) => p.id))}
            >
              {allPlaylists ? "Снять все плейлисты" : "Выбрать все плейлисты"}
            </button>
          {/if}
        </div>

        <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
          {#if preview}
            {@render sourceRow(pick.liked, t.library.liked, tracks(preview.liked), IconHeart, (v) => (pick.liked = v))}
            {@render sourceRow(
              pick.albums,
              "Сохранённые альбомы",
              `${preview.albums} ${plural(preview.albums, "альбом", "альбома", "альбомов")}`,
              IconAlbum,
              (v) => (pick.albums = v),
            )}
            {@render sourceRow(
              pick.artists,
              "Исполнители из библиотеки",
              `${preview.artists} ${plural(preview.artists, "исполнитель", "исполнителя", "исполнителей")} · вся дискография`,
              IconArtist,
              (v) => (pick.artists = v),
            )}
            {#each preview.playlists as pl (pl.id)}
              <label class="flex h-[60px] items-center gap-4 px-5 transition-colors hover:bg-fill-1">
                {@render checkbox(pick.playlistIds.includes(pl.id), pl.title, (v) => togglePlaylist(pl.id, v))}
                <Cover coverKey={pl.coverKey} size={40} rounded="sm" />
                <div class="min-w-0 flex-1">
                  <div class="truncate text-[14px]">{pl.title}</div>
                  <div class="truncate text-[12.5px] text-ink-3">Плейлист · {tracks(pl.trackCount)}</div>
                </div>
              </label>
            {/each}
          {:else if previewError}
            <div class="flex items-center gap-4 px-5 py-4">
              <p class="min-w-0 flex-1 text-[13px] leading-snug text-ink-3">{previewError}</p>
              <button
                type="button"
                class="h-9 shrink-0 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3"
                onclick={() => downloads.loadPreview()}
              >
                {t.common.retry}
              </button>
            </div>
          {:else}
            {#each [0, 1, 2, 3] as i (i)}
              <div class="flex h-[60px] items-center gap-4 px-5">
                <div class="size-[18px] animate-pulse rounded-[5px] bg-fill-2"></div>
                <div class="size-[40px] animate-pulse rounded-sm bg-fill-2"></div>
                <div class="h-3 w-1/3 animate-pulse rounded bg-fill-2"></div>
              </div>
            {/each}
          {/if}
        </div>

        <div class="mt-4 flex items-center gap-4">
          <button
            type="button"
            disabled={!preview || nothingChosen || starting || !!sync}
            class="flex h-9 items-center gap-2 rounded-full bg-ink px-5 text-[13px] font-semibold text-black transition-[background-color,opacity]
              hover:bg-[#dcdcdc] disabled:opacity-40 disabled:hover:bg-ink"
            {@attach pressable(0.95)}
            onclick={start}
          >
            <IconDownload class="size-4" />{sync ? "Идёт синхронизация…" : "Синхронизировать"}
          </button>
          <span class="text-[12.5px] leading-snug text-ink-3">Скачиваются только новые треки, состав плейлистов обновляется.</span>
        </div>
      </section>
    {:else}
      <section class="mb-8 flex items-center gap-4 rounded-lg bg-white/[0.04] px-5 py-4">
        <div class="min-w-0 flex-1">
          <div class="text-[14.5px] font-medium">YouTube Music не подключён</div>
          <div class="mt-0.5 text-[13px] leading-snug text-ink-3">Подключите аккаунт, чтобы выбрать лайки, плейлисты, альбомы и исполнителей.</div>
        </div>
        <button
          type="button"
          class="h-9 shrink-0 rounded-full bg-ink px-4 text-[13px] font-semibold text-black transition-[background-color] hover:bg-[#dcdcdc]"
          onclick={() => (downloads.connectOpen = true)}
        >
          Подключить
        </button>
      </section>
    {/if}

    {#if downloads.failed.length > 0}
      <section>
        <div class="mb-3 flex items-center justify-between">
          <h2 class="text-[13px] font-semibold tracking-wide text-ink-3 uppercase">
            Не удалось скачать · <span class="tnum">{downloads.failed.length}</span>
          </h2>
          <button
            type="button"
            class="h-8 rounded-full bg-fill-2 px-3.5 text-[12.5px] font-medium text-ink transition-colors hover:bg-fill-3"
            onclick={() => retry("all-failed")}
          >
            Повторить все
          </button>
        </div>
        <div class="-mx-3">
          {#each downloads.failed as job (job.id)}
            <div class="group grid h-[52px] grid-cols-[40px_minmax(0,1fr)_minmax(0,1.2fr)_auto] items-center gap-3 rounded-md px-3 transition-colors duration-100 hover:bg-fill-2">
              <div class="grid size-[40px] place-items-center rounded-sm bg-fill-2 text-ink-3"><IconAlert class="size-[18px]" /></div>
              <div class="min-w-0">
                <div class="truncate text-[14px]">{job.title}</div>
                <div class="truncate text-[12.5px] text-ink-3">{job.artists}</div>
              </div>
              <div class="min-w-0 truncate text-[13px] text-ink-3" data-tip={`${job.error} · попыток: ${job.attempts}`}>{job.error}</div>
              <button
                type="button"
                class="h-8 rounded-full px-3.5 text-[12.5px] font-medium text-ink-2 transition-colors hover:bg-fill-3 hover:text-ink"
                onclick={() => retry(job.id)}
              >
                {t.common.retry}
              </button>
            </div>
          {/each}
        </div>
      </section>
    {/if}
  {/if}
</div>

<style>
  /* A sliding segment for work of unknown length. */
  .indeterminate {
    animation: slide 1.2s cubic-bezier(0.45, 0, 0.55, 1) infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(300%);
    }
  }
</style>
