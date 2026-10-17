<script lang="ts">
  import type { Snippet } from "svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { t } from "$lib/i18n/ru";
  import Switch from "$lib/ui/Switch.svelte";
  import Slider from "$lib/ui/Slider.svelte";
  import { coverUrl, invoke } from "$lib/ipc";
  import { prompt, toasts } from "$lib/stores/overlay.svelte";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { router } from "$lib/stores/router.svelte";

  async function importFolder() {
    const path = await prompt.ask("Путь к папке с .webm / .m4a / .opus");
    if (!path) return;
    const n = await invoke<number>("dev_import_folder", { path });
    toasts.push(`Импортировано треков: ${n}`);
  }

  async function disconnect() {
    try {
      await downloads.disconnect();
      toasts.push("Аккаунт отключён. Скачанная музыка остаётся в библиотеке");
    } catch (e) {
      toasts.push(e instanceof Error ? e.message : "Не удалось отключить аккаунт", "error");
    }
  }

  const syncedAt = (ms: number) =>
    new Date(ms).toLocaleString("ru-RU", { day: "numeric", month: "long", hour: "2-digit", minute: "2-digit" });

  const account = $derived(downloads.account);
  let avatarFailed = $state(false);
  const p = $derived(prefs.value);
  let fadePreview = $state<number | null>(null);
</script>

{#snippet row(title: string, hint: string, control: Snippet)}
  <div class="flex items-center gap-6 px-5 py-4">
    <div class="min-w-0 flex-1">
      <div class="text-[14.5px] font-medium">{title}</div>
      <div class="mt-0.5 text-[13px] leading-snug text-ink-3">{hint}</div>
    </div>
    {@render control()}
  </div>
{/snippet}

<div class="max-w-[760px] px-8 pt-10 pb-12">
  <h1 class="mb-8 text-[26px] font-bold tracking-tight">{t.nav.settings}</h1>

  <section class="mb-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">Аккаунт YouTube Music</h2>
    <div class="flex items-center gap-4 rounded-lg bg-white/[0.04] px-5 py-4">
      {#if account.connected}
        {@const avatar = coverUrl(account.avatar, 64)}
        <div class="grid size-[40px] shrink-0 place-items-center overflow-hidden rounded-full bg-fill-3 text-[15px] font-semibold">
          {#if avatar && !avatarFailed}
            <img src={avatar} alt="" class="size-full object-cover" draggable="false" onerror={() => (avatarFailed = true)} />
          {:else}
            {(account.name ?? "?").slice(0, 1).toUpperCase()}
          {/if}
        </div>
        <div class="min-w-0 flex-1">
          <div class="truncate text-[14.5px] font-medium">{account.name ?? "Аккаунт подключён"}</div>
          <div class="mt-0.5 text-[13px] leading-snug text-ink-3">
            {account.lastSyncAt ? `Последняя синхронизация ${syncedAt(account.lastSyncAt)}` : "Ещё не синхронизировался"}
          </div>
        </div>
        <button
          type="button"
          class="h-9 shrink-0 rounded-full px-4 text-[13px] font-medium text-ink-2 transition-colors hover:bg-fill-2 hover:text-ink"
          onclick={disconnect}
        >
          Отключить
        </button>
        <button
          type="button"
          class="h-9 shrink-0 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3"
          onclick={() => router.go({ name: "downloads" })}
        >
          Синхронизация
        </button>
      {:else}
        <div class="min-w-0 flex-1">
          <div class="text-[14.5px] font-medium">Не подключён</div>
          <div class="mt-0.5 text-[13px] leading-snug text-ink-3">
            Нужен, чтобы выкачать лайки, плейлисты, альбомы и исполнителей. Данные входа хранятся только на этом компьютере.
          </div>
        </div>
        <button
          type="button"
          class="h-9 shrink-0 rounded-full bg-ink px-4 text-[13px] font-semibold text-black transition-[background-color] hover:bg-[#dcdcdc]"
          onclick={() => (downloads.connectOpen = true)}
        >
          Подключить
        </button>
      {/if}
    </div>
  </section>

  <section class="mb-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">Воспроизведение</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet crossfadeSwitch()}
        <Switch checked={p.crossfade} label="Плавный переход между треками" onchange={(v) => prefs.update({ crossfade: v })} />
      {/snippet}
      {@render row(
        "Плавный переход между треками",
        "Конец трека плавно смешивается с началом следующего. Треки одного альбома идут без паузы, как задумано.",
        crossfadeSwitch,
      )}
      {#if p.crossfade}
        {#snippet fadeSlider()}
          <div class="flex w-56 items-center gap-3">
            <Slider
              class="flex-1"
              label="Длительность перехода"
              value={fadePreview ?? p.crossfadeSec}
              max={12}
              wheelStep={1}
              oninput={(v) => (fadePreview = Math.max(1, Math.round(v)))}
              onchange={(v) => {
                fadePreview = null;
                void prefs.update({ crossfadeSec: Math.max(1, Math.round(v)) });
              }}
            />
            <span class="w-9 text-right text-[13px] text-ink-2 tnum">{fadePreview ?? p.crossfadeSec} с</span>
          </div>
        {/snippet}
        {@render row("Длительность перехода", "От 1 до 12 секунд. При ручном переключении — не больше 2 секунд.", fadeSlider)}
      {/if}
    </div>
  </section>

  <section>
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">Внешний вид</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet bgSwitch()}
        <Switch checked={p.smoothBackground} label="Плавная смена фона" onchange={(v) => prefs.update({ smoothBackground: v })} />
      {/snippet}
      {@render row(
        "Плавная смена фона",
        "Подсветка от обложки и фон полноэкранного плеера плавно перетекают в цвета нового трека.",
        bgSwitch,
      )}
    </div>
  </section>

  <section class="mt-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">Библиотека</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet importButton()}
        <button
          type="button"
          class="h-9 shrink-0 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3"
          onclick={importFolder}
        >
          Выбрать папку…
        </button>
      {/snippet}
      {@render row(
        "Импорт папки с аудио",
        "Добавляет в библиотеку файлы .webm, .m4a и .opus из папки. Обложка берётся из картинки с тем же именем.",
        importButton,
      )}
    </div>
  </section>
</div>
