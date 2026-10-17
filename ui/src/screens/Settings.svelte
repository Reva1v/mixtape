<script lang="ts">
  import type { Snippet } from "svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { t } from "$lib/i18n/ru";
  import Switch from "$lib/ui/Switch.svelte";
  import Slider from "$lib/ui/Slider.svelte";

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
</div>
