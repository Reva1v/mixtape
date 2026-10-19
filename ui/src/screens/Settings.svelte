<script lang="ts">
  import type { Component, Snippet } from "svelte";
  import IconCrossfade from "~icons/solar/soundwave-linear";
  import IconDuration from "~icons/solar/stopwatch-linear";
  import IconSmart from "~icons/solar/magic-stick-3-linear";
  import IconFullscreen from "~icons/solar/maximize-square-3-linear";
  import IconNormalize from "~icons/solar/tuning-2-linear";
  import IconCache from "~icons/solar/database-linear";
  import IconAutosave from "~icons/solar/download-minimalistic-linear";
  import IconClear from "~icons/solar/trash-bin-trash-linear";
  import IconFolder from "~icons/solar/folder-linear";
  import IconLanguage from "~icons/solar/globus-linear";
  import IconIconStyle from "~icons/solar/pallete-2-linear";
  import IconBackground from "~icons/solar/gallery-wide-linear";
  import IconTray from "~icons/solar/minimize-square-linear";
  import IconAutostart from "~icons/solar/power-linear";
  import IconUpdate from "~icons/solar/refresh-circle-linear";
  import IconAutoUpdate from "~icons/solar/cloud-download-linear";
  import IconImport from "~icons/solar/folder-with-files-linear";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { LANGS, locale, t } from "$lib/i18n";
  import Select from "$lib/ui/Select.svelte";
  import { navIcons } from "$lib/ui/navIcons";
  import { updates } from "$lib/stores/updates.svelte";
  import Switch from "$lib/ui/Switch.svelte";
  import Slider from "$lib/ui/Slider.svelte";
  import IntegrationsSettings from "$lib/ui/IntegrationsSettings.svelte";
  import { coverUrl, invoke } from "$lib/ipc";
  import { prompt, toasts } from "$lib/stores/overlay.svelte";
  import { downloads } from "$lib/stores/downloads.svelte";
  import { router } from "$lib/stores/router.svelte";
  import type { StorageInfo } from "$lib/ipc/bindings/StorageInfo";
  import type { Autostart } from "$lib/ipc/bindings/Autostart";

  async function importFolder() {
    const path = await prompt.ask(t.libraryScreen.importPrompt);
    if (!path) return;
    const n = await invoke<number>("dev_import_folder", { path });
    toasts.push(t.libraryScreen.imported(n));
  }

  async function disconnect() {
    try {
      await downloads.disconnect();
      toasts.push(t.settings.account.disconnected);
    } catch (e) {
      toasts.push(e instanceof Error ? e.message : t.settings.account.disconnectFailed, "error");
    }
  }

  const syncedAt = (ms: number) =>
    new Date(ms).toLocaleString(locale.tag, { day: "numeric", month: "long", hour: "2-digit", minute: "2-digit" });

  const account = $derived(downloads.account);
  let avatarFailed = $state(false);
  const p = $derived(prefs.value);

  let storage = $state<StorageInfo | null>(null);
  void invoke<StorageInfo>("storage_get").then((v) => (storage = v));
  const GB = 1024 ** 3;
  const LIMITS = [1, 2, 5, 10, 20];
  const PLAYS = [2, 3, 5, 10];
  const gb = (b: number) =>
    b >= GB
      ? t.settings.storage.gb((b / GB).toLocaleString(locale.tag, { maximumFractionDigits: 1 }))
      : t.settings.storage.mb(String(Math.round(b / 1024 ** 2)));

  async function storageCall(cmd: string, args?: Record<string, unknown>) {
    try {
      storage = await invoke<StorageInfo>(cmd, args);
    } catch (e) {
      toasts.push(e instanceof Error ? e.message : t.settings.failed, "error");
    }
  }
  let fadePreview = $state<number | null>(null);

  // One choice over two prefs: smart transitions fall back to the crossfade until a track
  // is analysed, so «smart» keeps the crossfade on too.
  const TRANSITIONS = ["gapless", "smooth", "smart"] as const;
  type Transition = (typeof TRANSITIONS)[number];
  const transition = $derived<Transition>(p.smartTransitions ? "smart" : p.crossfade ? "smooth" : "gapless");
  function setTransition(m: Transition) {
    void prefs.update({ crossfade: m !== "gapless", smartTransitions: m === "smart" });
  }

  // Start with Windows lives in the registry, not in prefs: read it each time the page opens.
  let autostart = $state<Autostart>("off");
  void invoke<Autostart>("autostart_get").then((v) => (autostart = v));
  async function setAutostart(mode: Autostart) {
    try {
      autostart = await invoke<Autostart>("autostart_set", { mode });
    } catch (e) {
      toasts.push(e instanceof Error && e.message ? e.message : t.tray.autostartFailed, "error");
      autostart = await invoke<Autostart>("autostart_get").catch(() => autostart);
    }
  }
</script>

{#snippet row(title: string, hint: string, control: Snippet, Icon?: Component)}
  <div class="flex items-center gap-6 px-5 py-4">
    {#if Icon}
      <div class="-mr-2 grid size-9 shrink-0 place-items-center rounded-[9px] bg-white/[0.06] text-ink-2">
        <Icon class="size-[19px]" />
      </div>
    {/if}
    <div class="min-w-0 flex-1">
      <div class="text-[14.5px] font-medium">{title}</div>
      <div class="mt-0.5 text-[13px] leading-snug text-ink-3">{hint}</div>
    </div>
    {@render control()}
  </div>
{/snippet}

<div class="mx-auto w-full max-w-[824px] px-8 pt-10 pb-12">
  <h1 class="mb-8 text-[26px] font-bold tracking-tight">{t.nav.settings}</h1>

  <section class="mb-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.settings.account.title}</h2>
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
          <div class="truncate text-[14.5px] font-medium">{account.name ?? t.settings.account.connected}</div>
          <div class="mt-0.5 text-[13px] leading-snug text-ink-3">
            {account.lastSyncAt ? t.settings.account.lastSync(syncedAt(account.lastSyncAt)) : t.settings.account.neverSynced}
          </div>
        </div>
        <button
          type="button"
          class="h-9 shrink-0 rounded-full px-4 text-[13px] font-medium text-ink-2 transition-colors hover:bg-fill-2 hover:text-ink"
          onclick={disconnect}
        >
          {t.settings.account.disconnect}
        </button>
        <button
          type="button"
          class="h-9 shrink-0 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3"
          onclick={() => router.go({ name: "downloads" })}
        >
          {t.settings.account.sync}
        </button>
      {:else}
        <div class="min-w-0 flex-1">
          <div class="text-[14.5px] font-medium">{t.settings.account.notConnected}</div>
          <div class="mt-0.5 text-[13px] leading-snug text-ink-3">
            {t.settings.account.notConnectedHint}
          </div>
        </div>
        <button
          type="button"
          class="h-9 shrink-0 rounded-full bg-ink px-4 text-[13px] font-semibold text-black transition-[background-color] hover:bg-[#dcdcdc]"
          onclick={() => (downloads.connectOpen = true)}
        >
          {t.settings.account.connect}
        </button>
      {/if}
    </div>
  </section>

  <section class="mb-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.settings.playback.title}</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet transitionChoice()}
        <div class="flex shrink-0 gap-1 rounded-full bg-fill-1 p-1 ring-1 ring-line" role="radiogroup" aria-label={t.transition.title}>
          {#each TRANSITIONS as m (m)}
            <button
              type="button"
              role="radio"
              aria-checked={transition === m}
              class="h-7 rounded-full px-3 text-[12.5px] font-medium whitespace-nowrap transition-colors
                {transition === m ? 'bg-ink text-black' : 'text-ink-2 hover:bg-fill-2 hover:text-ink'}"
              onclick={() => setTransition(m)}
            >
              {t.transition[m]}
            </button>
          {/each}
        </div>
      {/snippet}
      {@render row(
        t.transition.title,
        t.transition[`${transition}Hint`],
        transitionChoice,
        transition === "smart" ? IconSmart : IconCrossfade,
      )}
      {#if transition !== "gapless"}
        {#snippet fadeSlider()}
          <div class="flex w-56 items-center gap-3">
            <Slider
              class="flex-1"
              label={t.settings.playback.duration}
              value={fadePreview ?? p.crossfadeSec}
              max={12}
              wheelStep={1}
              oninput={(v) => (fadePreview = Math.max(1, Math.round(v)))}
              onchange={(v) => {
                fadePreview = null;
                void prefs.update({ crossfadeSec: Math.max(1, Math.round(v)) });
              }}
            />
            <span class="w-9 text-right text-[13px] text-ink-2 tnum">{t.settings.playback.seconds(fadePreview ?? p.crossfadeSec)}</span>
          </div>
        {/snippet}
        {@render row(t.settings.playback.duration, t.settings.playback.durationHint, fadeSlider, IconDuration)}
      {/if}
      {#snippet mixFullSwitch()}
        <Switch
          checked={p.mixFullscreen}
          label={t.settings.playback.mixFullscreen}
          onchange={(v) => prefs.update({ mixFullscreen: v })}
        />
      {/snippet}
      {@render row(
        t.settings.playback.mixFullscreen,
        t.settings.playback.mixFullscreenHint,
        mixFullSwitch,
        IconFullscreen,
      )}
      {#snippet normalizeSwitch()}
        <Switch checked={p.normalizeVolume} label={t.normalize.title} onchange={(v) => prefs.update({ normalizeVolume: v })} />
      {/snippet}
      {@render row(t.normalize.title, t.normalize.hint, normalizeSwitch, IconNormalize)}
    </div>
  </section>

  <IntegrationsSettings {row} />

  {#snippet choice(options: number[], current: number | undefined, label: (n: number) => string, pick: (n: number) => void)}
    <div class="flex shrink-0 gap-1 rounded-full bg-fill-1 p-1 ring-1 ring-line">
      {#each options as n (n)}
        <button
          type="button"
          class="h-7 rounded-full px-3 text-[12.5px] font-medium transition-colors
            {current === n ? 'bg-ink text-black' : 'text-ink-2 hover:bg-fill-2 hover:text-ink'}"
          onclick={() => pick(n)}
        >
          {label(n)}
        </button>
      {/each}
    </div>
  {/snippet}

  {#if storage}
    {@const st = storage}
    <section class="mb-8">
      <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.settings.storage.title}</h2>
      <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
        {#snippet limitChoice()}
          {@render choice(LIMITS, Math.round(st.cacheLimit / GB), (n) => t.settings.storage.gb(String(n)), (n) =>
            void storageCall("storage_set_cache_limit", { bytes: n * GB }),
          )}
        {/snippet}
        {@render row(
          t.settings.storage.cache,
          t.settings.storage.cacheHint(gb(st.cacheBytes)),
          limitChoice,
          IconCache,
        )}
        {#snippet autosaveSwitch()}
          <div class="flex items-center gap-4">
            {#if st.autosavePlays}
              {@render choice(PLAYS, st.autosavePlays, (n) => `${n}×`, (n) => void storageCall("storage_set_autosave", { plays: n }))}
            {/if}
            <Switch
              checked={!!st.autosavePlays}
              label={t.settings.storage.autosaveLabel}
              onchange={(v) => void storageCall("storage_set_autosave", { plays: v ? 3 : null })}
            />
          </div>
        {/snippet}
        {@render row(
          t.settings.storage.autosave,
          st.autosavePlays ? t.settings.storage.autosaveOn(st.autosavePlays) : t.settings.storage.autosaveOff,
          autosaveSwitch,
          IconAutosave,
        )}
        {#snippet clearButton()}
          <button
            type="button"
            class="h-9 shrink-0 rounded-full bg-fill-2 px-4 text-[13px] font-semibold ring-1 ring-line transition-colors hover:bg-fill-3"
            onclick={() => void storageCall("cache_clear").then(() => toasts.push(t.settings.storage.cleared))}
          >
            {t.settings.storage.clearButton}
          </button>
        {/snippet}
        {@render row(t.settings.storage.clear, t.settings.storage.clearHint, clearButton, IconClear)}
        {#snippet openButton()}
          <button
            type="button"
            class="h-9 shrink-0 rounded-full bg-fill-2 px-4 text-[13px] font-semibold ring-1 ring-line transition-colors hover:bg-fill-3"
            onclick={() => void invoke("open_data_dir")}
          >
            {t.settings.storage.open}
          </button>
        {/snippet}
        {@render row(t.settings.storage.dataDir, st.dataDir, openButton, IconFolder)}
      </div>
    </section>
  {/if}

  <section>
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.settings.appearance.title}</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet langSelect()}
        <Select
          value={locale.lang}
          options={LANGS.map((l) => ({ id: l.id, label: l.name }))}
          label={t.language.title}
          onchange={(language) => prefs.update({ language })}
        />
      {/snippet}
      {@render row(t.language.title, t.language.hint, langSelect, IconLanguage)}
      {#snippet iconSelect()}
        <Select
          value={p.iconStyle}
          options={(["outline", "solid", "duotone", "rounded"] as const).map((id) => ({ id, label: t.iconStyle[id], icon: navIcons[id].home }))}
          label={t.iconStyle.title}
          onchange={(iconStyle) => prefs.update({ iconStyle })}
        />
      {/snippet}
      {@render row(t.iconStyle.title, t.iconStyle.hint, iconSelect, IconIconStyle)}
      {#snippet bgSwitch()}
        <Switch checked={p.smoothBackground} label={t.settings.appearance.smoothBackground} onchange={(v) => prefs.update({ smoothBackground: v })} />
      {/snippet}
      {@render row(
        t.settings.appearance.smoothBackground,
        t.settings.appearance.smoothBackgroundHint,
        bgSwitch,
        IconBackground,
      )}
    </div>
  </section>

  <section class="mt-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.tray.section}</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet traySwitch()}
        <Switch checked={p.closeToTray} label={t.tray.closeToTray} onchange={(v) => prefs.update({ closeToTray: v })} />
      {/snippet}
      {@render row(t.tray.closeToTray, t.tray.closeToTrayHint, traySwitch, IconTray)}
      {#snippet autostartSelect()}
        <Select
          value={autostart}
          options={[
            { id: "off", label: t.tray.autostartOff },
            { id: "on", label: t.tray.autostartOn },
            { id: "hidden", label: t.tray.autostartHidden },
          ]}
          label={t.tray.autostart}
          onchange={(mode) => void setAutostart(mode)}
        />
      {/snippet}
      {@render row(t.tray.autostart, t.tray.autostartHint, autostartSelect, IconAutostart)}
    </div>
  </section>

  <section class="mt-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.updates.title}</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet updateButton()}
        {@const s = updates.status}
        {#if s.kind === "available"}
          <button
            type="button"
            class="h-9 shrink-0 rounded-full bg-ink px-4 text-[13px] font-semibold text-black transition-opacity hover:opacity-90"
            onclick={() => updates.install()}
          >
            {t.updates.install}
          </button>
        {:else}
          <button
            type="button"
            disabled={s.kind === "checking" || s.kind === "installing"}
            class="h-9 shrink-0 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3 disabled:opacity-60"
            onclick={() => updates.check()}
          >
            {s.kind === "checking" ? t.updates.checking : t.updates.check}
          </button>
        {/if}
      {/snippet}
      {@render row(
        t.updates.version(updates.version),
        (() => {
          const s = updates.status;
          if (s.kind === "latest") return t.updates.latest;
          if (s.kind === "available") return `${t.updates.available(s.info.version)}. ${t.updates.restartHint}`;
          if (s.kind === "installing") return t.updates.installing(s.total ? Math.round((s.done / s.total) * 100) : null);
          if (s.kind === "error") return s.message;
          return t.updates.restartHint;
        })(),
        updateButton,
        IconUpdate,
      )}
      {#snippet autoSwitch()}
        <Switch checked={p.autoUpdate} label={t.updates.auto} onchange={(v) => prefs.update({ autoUpdate: v })} />
      {/snippet}
      {@render row(t.updates.auto, t.updates.autoHint, autoSwitch, IconAutoUpdate)}
    </div>
  </section>

  <section class="mt-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.nav.library}</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet importButton()}
        <button
          type="button"
          class="h-9 shrink-0 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3"
          onclick={importFolder}
        >
          {t.settings.library.chooseFolder}
        </button>
      {/snippet}
      {@render row(
        t.settings.library.importFolder,
        t.settings.library.importHint,
        importButton,
        IconImport,
      )}
    </div>
  </section>
</div>
