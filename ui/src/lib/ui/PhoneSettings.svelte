<script lang="ts">
  // Settings → «Phone»: pairing with Mixtape on the phone for the stats sync (specs 007).
  // Not paired: «Pair phone» shows a QR code for 5 minutes. Paired: the phone's name, the
  // last exchange and «Unpair». Rows come from the Settings screen.
  import type { Component, Snippet } from "svelte";
  import IconPhone from "~icons/solar/smartphone-linear";
  import IconFirewall from "~icons/solar/shield-warning-linear";
  import { locale, t } from "$lib/i18n";
  import { onPeerStatus, peerFirewallAdd, peerPairCancel, peerPairStart, peerStatus, peerUnpair } from "$lib/ipc/peer";
  import type { PairStart, PeerStatus } from "$lib/ipc/peer";
  import { toasts } from "$lib/stores/overlay.svelte";

  let { row }: { row: Snippet<[string, string, Snippet, Component?]> } = $props();

  let status = $state<PeerStatus | null>(null);
  let code = $state.raw<PairStart | null>(null);
  let now = $state(Date.now());
  let busy = $state(false);
  let confirming = $state(false);

  const errorText = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);

  function apply(s: PeerStatus) {
    // The phone scanned the code: the pairing replaces the QR code.
    if (code && s.paired && !status?.paired) {
      code = null;
      toasts.push(t.phone.paired(s.peerName ?? ""));
    }
    if (!s.paired) confirming = false;
    status = s;
  }

  $effect(() => {
    void peerStatus().then(apply).catch(() => {});
    const un = onPeerStatus(apply);
    return () => void un.then((f) => f());
  });

  // Leaving the page drops the code (the PC stops listening unless a phone is paired).
  $effect(() => () => {
    if (code) void peerPairCancel().catch(() => {});
  });

  // The countdown.
  $effect(() => {
    if (!code) return;
    now = Date.now();
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  const left = $derived(code ? Math.max(0, code.expiresAt - now) : 0);
  const expired = $derived(!!code && left === 0);
  const leftText = $derived.by(() => {
    const s = Math.ceil(left / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  });

  async function start() {
    busy = true;
    try {
      code = await peerPairStart();
      // The firewall check ran with it.
      status = await peerStatus();
    } catch (e) {
      toasts.push(errorText(e, t.phone.failed), "error");
    } finally {
      busy = false;
    }
  }

  async function cancel() {
    code = null;
    try {
      apply(await peerPairCancel());
    } catch {
      // The status event catches up.
    }
  }

  async function allowFirewall() {
    busy = true;
    try {
      apply(await peerFirewallAdd());
      toasts.push(t.phone.firewallAdded);
    } catch (e) {
      toasts.push(errorText(e, t.settings.failed), "error");
    } finally {
      busy = false;
    }
  }

  async function unpair() {
    busy = true;
    try {
      apply(await peerUnpair());
      toasts.push(t.phone.unpaired);
    } catch (e) {
      toasts.push(errorText(e, t.settings.failed), "error");
    } finally {
      busy = false;
      confirming = false;
    }
  }

  const when = (ms: number) =>
    new Date(ms).toLocaleString(locale.tag, { day: "numeric", month: "long", hour: "2-digit", minute: "2-digit" });

  const btn = "h-9 shrink-0 rounded-full px-4 text-[13px] font-medium transition-colors disabled:opacity-50";
  const primary = `${btn} bg-ink font-semibold text-black hover:bg-[#dcdcdc]`;
  const quiet = `${btn} text-ink-2 hover:bg-fill-2 hover:text-ink`;
  const filled = `${btn} bg-fill-2 text-ink hover:bg-fill-3`;
</script>

{#if status}
  {@const s = status}
  <section class="mb-8">
    <h2 class="mb-3 text-[13px] font-semibold tracking-wide text-ink-3 uppercase">{t.phone.title}</h2>
    <div class="divide-y divide-line overflow-hidden rounded-lg bg-white/[0.04]">
      {#if s.paired}
        {#snippet pairedControl()}
          {#if confirming}
            <div class="flex shrink-0 gap-2">
              <button type="button" class={quiet} disabled={busy} onclick={() => (confirming = false)}>{t.phone.cancel}</button>
              <button type="button" class={filled} disabled={busy} onclick={unpair}>{t.phone.unpair}</button>
            </div>
          {:else}
            <button type="button" class={quiet} disabled={busy} onclick={() => (confirming = true)}>{t.phone.unpair}</button>
          {/if}
        {/snippet}
        {@render row(
          t.phone.pairedWith(s.peerName ?? ""),
          confirming
            ? t.phone.unpairConfirm
            : !s.listening
              ? t.phone.notListening
              : s.lastExchange
                ? t.phone.lastExchange(when(s.lastExchange))
                : t.phone.neverExchanged,
          pairedControl,
          IconPhone,
        )}
      {:else if code}
        {@const c = code}
        <div class="flex items-center gap-6 px-5 py-5">
          <!-- Black on white with the quiet zone, whatever the theme: phones read it best. -->
          <div
            class="size-[188px] shrink-0 overflow-hidden rounded-[10px] bg-white transition-opacity [&>svg]:block [&>svg]:size-full
              {expired ? 'opacity-15' : ''}"
            role="img"
            aria-label={t.phone.scanTitle}
          >
            {@html c.svg}
          </div>
          <div class="min-w-0 flex-1">
            <div class="text-[14.5px] font-medium">{t.phone.scanTitle}</div>
            <div class="mt-1 text-[13px] leading-snug text-ink-3">{t.phone.scanHint}</div>
            <div class="mt-3 text-[13px] font-medium tnum {expired ? 'text-ink' : 'text-ink-2'}">
              {expired ? t.phone.expired : t.phone.expiresIn(leftText)}
            </div>
            <div class="mt-4 flex gap-2">
              <button type="button" class={expired ? primary : filled} disabled={busy} onclick={start}>{t.phone.newCode}</button>
              <button type="button" class={quiet} disabled={busy} onclick={cancel}>{t.phone.cancel}</button>
            </div>
          </div>
        </div>
      {:else}
        {#snippet pairButton()}
          <button type="button" class={primary} disabled={busy} onclick={start}>{t.phone.pair}</button>
        {/snippet}
        {@render row(t.phone.notPaired, t.phone.notPairedHint, pairButton, IconPhone)}
      {/if}
      {#if s.firewall === "missing" && (code || s.paired)}
        {#snippet firewallButton()}
          <button type="button" class={filled} disabled={busy} onclick={allowFirewall}>{t.phone.firewallAllow}</button>
        {/snippet}
        {@render row(t.phone.firewall, t.phone.firewallHint, firewallButton, IconFirewall)}
      {/if}
    </div>
  </section>
{/if}
