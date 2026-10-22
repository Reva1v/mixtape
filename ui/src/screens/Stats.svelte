<script lang="ts" module>
  import type { ListeningStats as Cached } from "$lib/ipc/bindings/ListeningStats";
  import type { StatsPeriod as Period } from "$lib/ipc/bindings/StatsPeriod";
  // The last recap of each period and device and the ones chosen: coming back renders at
  // full height at once; a fresh copy replaces it quietly.
  const cache = new Map<string, Cached>();
  const key = (p: Period, device: string) => `${p}|${device}`;
  let lastPeriod: Period = "month";
  /** A device id (`DeviceTotal.device`), or "" for all devices. */
  let lastDevice = "";
  /** Names of the devices seen in any recap, so a chosen one stays in the list. */
  const deviceNames = new Map<string, { thisDevice: boolean; name?: string }>();

  /** One bar chart: a value, an axis label and a tooltip per bar. */
  export type Bars = { values: number[]; labels: string[]; tips: string[] };
</script>

<script lang="ts">
  import { invoke, on } from "$lib/ipc";
  import type { DeviceTotal } from "$lib/ipc/bindings/DeviceTotal";
  import type { ListeningStats } from "$lib/ipc/bindings/ListeningStats";
  import type { StatsPeriod } from "$lib/ipc/bindings/StatsPeriod";
  import type { TopArtist } from "$lib/ipc/bindings/TopArtist";
  import type { Track } from "$lib/ipc/bindings/Track";
  import { Fixed } from "$lib/stores/paged.svelte";
  import { glowOf, player } from "$lib/stores/player.svelte";
  import { layout } from "$lib/stores/layout.svelte";
  import { router } from "$lib/stores/router.svelte";
  import { openAlbum } from "$lib/stores/actions";
  import { t, locale } from "$lib/i18n";
  import { dragScroll } from "$lib/motion";
  import Select from "$lib/ui/Select.svelte";
  import TrackList from "$lib/ui/TrackList.svelte";
  import Card from "$lib/ui/Card.svelte";
  import Skeleton from "$lib/ui/Skeleton.svelte";
  import EmptyState from "$lib/ui/EmptyState.svelte";
  import { albumMenu, artistMenu } from "$lib/ui/collectionMenu";
  import IconChart from "~icons/solar/chart-2-bold";

  let period = $state<StatsPeriod>(lastPeriod);
  let device = $state(lastDevice);
  let stats = $state.raw<ListeningStats | null>(cache.get(key(lastPeriod, lastDevice)) ?? null);
  const top = new Fixed<Track>(cache.get(key(lastPeriod, lastDevice))?.topTracks.map((x) => x.track) ?? []);

  const periods = $derived<{ id: StatsPeriod; label: string }[]>([
    { id: "week", label: t.stats.period.week },
    { id: "month", label: t.stats.period.month },
    { id: "year", label: t.stats.period.year },
    { id: "all", label: t.stats.period.all },
  ]);

  async function load(p: StatsPeriod, d: string) {
    // Local hours and days: the core gets this computer's offset from UTC.
    const s = await invoke<ListeningStats>("stats_get", {
      period: p,
      tzOffsetMin: -new Date().getTimezoneOffset(),
      device: d || null,
    });
    cache.set(key(p, d), s);
    for (const x of s.byDevice) deviceNames.set(x.device, { thisDevice: x.thisDevice, name: x.name });
    if (p !== period || d !== device) return;
    stats = s;
    top.items = s.topTracks.map((x) => x.track);
  }

  $effect(() => {
    const p = period;
    const d = device;
    lastPeriod = p;
    lastDevice = d;
    const hit = cache.get(key(p, d));
    stats = hit ?? null;
    top.items = hit?.topTracks.map((x) => x.track) ?? [];
    void load(p, d);
  });

  // A new play in the log (or changed track rows): re-read once things settle.
  $effect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const later = () => {
      clearTimeout(timer);
      timer = setTimeout(() => void load(period, device), 1500);
    };
    const offs = [on("stats:changed", later), on("library:changed", later)];
    return () => {
      clearTimeout(timer);
      for (const off of offs) void off.then((f) => f());
    };
  });

  // The header takes the colours of the most played album.
  $effect(() => {
    const palette = stats?.topAlbums[0]?.album.palette ?? [];
    layout.pageGlow = palette.length ? glowOf(palette) : null;
  });
  $effect(() => () => (layout.pageGlow = null));

  // ---- Formatting ----

  const num = $derived(new Intl.NumberFormat(locale.tag));
  const plays = (n: number) => t.stats.playCount(n, num.format(n));

  /** "41 h 12 min" as number/unit pairs, so the numbers can be set larger. */
  function duration(ms: number): { value: string; unit: string }[] {
    const total = Math.round(ms / 60_000);
    const h = Math.floor(total / 60);
    const m = total % 60;
    if (h === 0) return [{ value: num.format(m), unit: t.stats.minutes }];
    if (h >= 100 || m === 0) return [{ value: num.format(h), unit: t.stats.hours }];
    return [
      { value: num.format(h), unit: t.stats.hours },
      { value: num.format(m), unit: t.stats.minutes },
    ];
  }
  const durationText = (ms: number) => duration(ms).map((p) => `${p.value} ${p.unit}`).join(" ");

  const hourFmt = $derived(new Intl.DateTimeFormat(locale.tag, { hour: "numeric" }));
  const dayFmt = $derived(new Intl.DateTimeFormat(locale.tag, { day: "numeric", month: "short" }));
  const weekdayFmt = $derived(new Intl.DateTimeFormat(locale.tag, { weekday: "long" }));
  const weekdayShort = $derived(new Intl.DateTimeFormat(locale.tag, { weekday: "short" }));

  const hourName = (h: number) => hourFmt.format(new Date(2024, 0, 1, h));
  // 1 January 2024 was a Monday; the core counts weekdays from Monday.
  const weekdays = $derived(Array.from({ length: 7 }, (_, i) => new Date(2024, 0, 1 + i)));

  const hours = $derived<Bars | null>(
    stats && {
      values: stats.byHour,
      labels: stats.byHour.map((_, h) => (h % 6 === 0 ? hourName(h) : "")),
      tips: stats.byHour.map((ms, h) => `${hourName(h)} · ${durationText(ms)}`),
    },
  );
  const days = $derived<Bars | null>(
    stats && {
      values: stats.byWeekday,
      labels: weekdays.map((d) => weekdayShort.format(d)),
      tips: stats.byWeekday.map((ms, i) => `${weekdayFmt.format(weekdays[i])} · ${durationText(ms)}`),
    },
  );

  // ---- Devices (stats synced with the phone) ----

  const deviceLabel = (x: { thisDevice: boolean; name?: string }) =>
    x.thisDevice ? t.stats.thisDevice : (x.name ?? t.stats.otherDevice);

  /** This device first, then by listening time. */
  const split = $derived<DeviceTotal[]>(
    [...(stats?.byDevice ?? [])].sort((a, b) => Number(b.thisDevice) - Number(a.thisDevice) || b.listenedMs - a.listenedMs),
  );
  const splitTotal = $derived(split.reduce((sum, x) => sum + x.listenedMs, 0));

  // Shown once plays came from more than one device (or a device is chosen).
  const deviceOptions = $derived.by<{ id: string; label: string }[]>(() => {
    if (split.length < 2 && !device) return [];
    const ids = [...new Set([...split.map((x) => x.device), ...(device ? [device] : [])])];
    return [
      { id: "", label: t.stats.allDevices },
      ...ids.map((id) => ({ id, label: deviceLabel(deviceNames.get(id) ?? { thisDevice: false }) })),
    ];
  });

  const asArtist = (a: TopArtist) => ({ id: a.id!, name: a.name, imageKey: a.imageKey, inLibrary: true, palette: [] });

  function playTop(i: number) {
    if (stats) void player.play({ kind: "tracks", trackIds: stats.topTracks.map((x) => x.track.id) }, i);
  }
</script>

{#snippet section(title: string)}
  <h2 class="mb-4 px-8 text-[20px] font-bold tracking-tight">{title}</h2>
{/snippet}

{#snippet tile(label: string, parts: { value: string; unit: string }[], sub?: string)}
  <div class="min-w-0 rounded-[10px] bg-white/[0.055] px-5 py-4">
    <div class="truncate text-[12.5px] text-ink-3">{label}</div>
    <div class="mt-2 flex items-baseline gap-1.5 truncate">
      {#each parts as p, i (i)}
        <span class="text-[30px] leading-none font-bold tracking-tight tnum">{p.value}</span>
        {#if p.unit}<span class="mr-1 text-[14px] font-medium text-ink-2">{p.unit}</span>{/if}
      {/each}
    </div>
    {#if sub}<div class="mt-2 truncate text-[12.5px] text-ink-3">{sub}</div>{/if}
  </div>
{/snippet}

{#snippet bars(title: string, b: Bars)}
  {@const max = Math.max(...b.values)}
  <div class="min-w-0 rounded-[10px] bg-white/[0.035] px-5 pt-4 pb-3.5">
    <h3 class="mb-4 text-[15px] font-semibold">{title}</h3>
    <div class="flex h-32 items-end gap-[3px]">
      {#each b.values as v, i (i)}
        <div class="flex h-full min-w-0 flex-1 cursor-default items-end" data-tip={b.tips[i]}>
          <div
            class="w-full rounded-[3px] {v > 0 && v === max ? 'bg-ink' : v > 0 ? 'bg-white/[0.22]' : 'bg-white/[0.06]'}"
            style:height={v > 0 ? `${Math.max(4, (v / max) * 100)}%` : "3px"}
          ></div>
        </div>
      {/each}
    </div>
    <div class="mt-2 flex gap-[3px] text-[11.5px] text-ink-3">
      {#each b.labels as l, i (i)}
        <div class="min-w-0 flex-1 overflow-visible text-center whitespace-nowrap">{l}</div>
      {/each}
    </div>
  </div>
{/snippet}

<div class="pb-12">
  <div class="flex items-end justify-between gap-4 px-8 pt-10 pb-6">
    <h1 class="text-[28px] font-bold tracking-tight">{t.stats.title}</h1>
    <div class="flex items-center gap-2">
      {#if deviceOptions.length}
        <Select bind:value={device} options={deviceOptions} label={t.stats.allDevices} />
      {/if}
      <Select bind:value={period} options={periods} label={t.stats.period.label} />
    </div>
  </div>

  {#if !stats}
    <div class="grid grid-cols-5 gap-3 px-8" aria-hidden="true">
      {#each [0, 1, 2, 3, 4] as k (k)}<Skeleton class="h-[92px] rounded-[10px]" />{/each}
    </div>
  {:else if stats.plays === 0}
    <EmptyState icon={IconChart} title={period === "all" ? t.stats.empty : t.stats.emptyPeriod} hint={t.stats.emptyHint}>
      {#if period !== "all"}
        <button
          type="button"
          class="h-9 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink-2 hover:bg-fill-3 hover:text-ink"
          onclick={() => (period = "all")}
        >
          {t.stats.showAll}
        </button>
      {/if}
    </EmptyState>
  {:else}
    <div class="{split.length > 1 ? 'mb-4' : 'mb-10'} grid grid-cols-[repeat(auto-fit,minmax(160px,1fr))] gap-3 px-8">
      {@render tile(t.stats.listened, duration(stats.listenedMs))}
      {@render tile(t.stats.plays, [{ value: num.format(stats.plays), unit: "" }])}
      {@render tile(t.stats.tracks, [{ value: num.format(stats.tracks), unit: "" }])}
      {@render tile(t.stats.artists, [{ value: num.format(stats.artists), unit: "" }])}
      {#if stats.topDay}
        {@render tile(
          t.stats.topDay,
          [{ value: dayFmt.format(new Date(stats.topDay.dayMs)), unit: "" }],
          `${weekdayFmt.format(new Date(stats.topDay.dayMs))} · ${durationText(stats.topDay.listenedMs)}`,
        )}
      {/if}
    </div>

    {#if split.length > 1}
      <!-- «This PC 26 h · Pixel 12 h · Total 38 h»: the period across all devices. -->
      <div class="mb-10 flex flex-wrap items-baseline gap-x-2 gap-y-1 px-8 text-[13px] text-ink-3">
        {#each split as x, i (x.device)}
          {#if i > 0}<span aria-hidden="true">·</span>{/if}
          <button
            type="button"
            class="rounded-sm transition-colors hover:text-ink {device === x.device ? 'text-ink' : ''}"
            onclick={() => (device = device === x.device ? "" : x.device)}
          >
            {deviceLabel(x)} <span class="font-medium text-ink-2 tnum">{durationText(x.listenedMs)}</span>
          </button>
        {/each}
        <span aria-hidden="true">·</span>
        <span>{t.stats.total} <span class="font-medium text-ink-2 tnum">{durationText(splitTotal)}</span></span>
      </div>
    {/if}

    <section class="mb-10">
      {@render section(t.stats.topTracks)}
      <TrackList rows={top} onplay={playTop} note={(i) => (stats ? plays(stats.topTracks[i]?.plays ?? 0) : undefined)} />
    </section>

    {#if stats.topArtists.length}
      <section class="mb-10">
        {@render section(t.stats.topArtists)}
        <div class="no-scrollbar -mt-2 flex snap-x gap-7 overflow-x-auto scroll-px-8 px-8 pt-2 pb-2" {@attach dragScroll()}>
          {#each stats.topArtists as a (a.id ?? `name:${a.name}`)}
            <div class="w-40 shrink-0 snap-start">
              <Card
                round
                coverKey={a.imageKey}
                title={a.name}
                subtitle={plays(a.plays)}
                onopen={a.id ? () => router.go({ name: "artist", id: a.id! }) : undefined}
                menu={a.id ? () => artistMenu(asArtist(a)) : undefined}
              />
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if stats.topAlbums.length}
      <section class="mb-10">
        {@render section(t.stats.topAlbums)}
        <div class="no-scrollbar -mt-2 flex snap-x gap-7 overflow-x-auto scroll-px-8 px-8 pt-2 pb-2" {@attach dragScroll()}>
          {#each stats.topAlbums as x (x.album.id)}
            <div class="w-44 shrink-0 snap-start">
              <Card
                coverKey={x.album.coverKey}
                title={x.album.title}
                artists={x.album.artists}
                meta={plays(x.plays)}
                onopen={() => openAlbum(x.album)}
                onplay={() => player.play({ kind: "album", id: x.album.id })}
                menu={() => albumMenu(x.album)}
                current={player.track?.album?.id === x.album.id}
              />
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <div class="grid grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)] gap-3 px-8">
      {#if hours}{@render bars(t.stats.byHour, hours)}{/if}
      {#if days}{@render bars(t.stats.byWeekday, days)}{/if}
    </div>
  {/if}
</div>
