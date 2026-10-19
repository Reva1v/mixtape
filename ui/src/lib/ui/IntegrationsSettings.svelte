<script lang="ts">
  // Settings → «Last.fm and Discord»: the Last.fm connection (desktop sign-in in the
  // browser) and the Discord presence switch. Rows come from the Settings screen.
  import type { Component, Snippet } from "svelte";
  import IconLastfm from "~icons/solar/radio-minimalistic-linear";
  import IconDiscord from "~icons/solar/gamepad-linear";
  import { t } from "$lib/i18n";
  import { invoke, on } from "$lib/ipc";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { toasts } from "$lib/stores/overlay.svelte";
  import Switch from "$lib/ui/Switch.svelte";
  import type { Integrations } from "$lib/ipc/bindings/Integrations";

  let { row }: { row: Snippet<[string, string, Snippet, Component?]> } = $props();

  let info = $state<Integrations | null>(null);
  let busy = $state(false);

  /** A sign-in was started here: its end (in the browser or by the button) gets a toast. */
  let announce = false;

  function announced(v: Integrations) {
    if (announce && v.lastfmUser) {
      announce = false;
      toasts.push(t.integrations.connected(v.lastfmUser));
    }
  }

  async function refresh() {
    info = await invoke<Integrations>("integrations_status");
    announced(info);
  }

  $effect(() => {
    void refresh().catch(() => {});
    const un = on("lastfm:changed", () => void refresh().catch(() => {}));
    return () => void un.then((f) => f());
  });

  async function call(cmd: string, done?: () => void) {
    busy = true;
    try {
      info = await invoke<Integrations>(cmd);
      done?.();
    } catch (e) {
      toasts.push(e instanceof Error ? e.message : t.integrations.failed, "error");
    } finally {
      busy = false;
    }
  }

  const connect = () => call("lastfm_connect", () => (announce = true));
  const confirm = () => call("lastfm_confirm", () => info && announced(info));
  const disconnect = () => call("lastfm_disconnect", () => toasts.push(t.integrations.disconnected));

  const btn = "h-9 shrink-0 rounded-full px-4 text-[13px] font-medium transition-colors disabled:opacity-50";
</script>

{#if info}
  {@const i = info}
  <section class="mb-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.integrations.title}</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#snippet lastfmControl()}
        {#if !i.lastfmAvailable}
          <span></span>
        {:else if i.lastfmUser}
          <button type="button" class="{btn} text-ink-2 hover:bg-fill-2 hover:text-ink" disabled={busy} onclick={disconnect}>
            {t.integrations.disconnect}
          </button>
        {:else if i.lastfmPending}
          <div class="flex shrink-0 gap-2">
            <button type="button" class="{btn} text-ink-2 hover:bg-fill-2 hover:text-ink" disabled={busy} onclick={connect}>
              {t.integrations.openAgain}
            </button>
            <button
              type="button"
              class="{btn} bg-ink font-semibold text-black hover:bg-[#dcdcdc]"
              disabled={busy}
              onclick={confirm}
            >
              {t.integrations.allowed}
            </button>
          </div>
        {:else}
          <button type="button" class="{btn} bg-ink font-semibold text-black hover:bg-[#dcdcdc]" disabled={busy} onclick={connect}>
            {t.integrations.connect}
          </button>
        {/if}
      {/snippet}
      {@render row(
        i.lastfmUser ? t.integrations.connectedAs(i.lastfmUser) : t.integrations.lastfm,
        !i.lastfmAvailable
          ? t.integrations.unavailable
          : i.lastfmUser
            ? t.integrations.connectedHint
            : i.lastfmPending
              ? t.integrations.pendingHint
              : t.integrations.hint,
        lastfmControl,
        IconLastfm,
      )}
      {#if i.discordAvailable}
        {#snippet discordSwitch()}
          <Switch
            checked={prefs.value.discordPresence}
            label={t.integrations.discord}
            onchange={(v) => prefs.update({ discordPresence: v })}
          />
        {/snippet}
        {@render row(t.integrations.discord, t.integrations.discordHint, discordSwitch, IconDiscord)}
      {/if}
    </div>
  </section>
{/if}
