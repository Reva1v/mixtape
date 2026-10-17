<script lang="ts">
  // The tray icon's menu (src-tauri/src/tray_menu.rs): the playing track with a like
  // button, prev / play / next, then the window items. The window stays hidden between
  // uses; on `tray-menu:open` the page reads a snapshot, measures the card and asks the
  // core to show it at the cursor. Blur, Esc or a window item clear the card first and
  // hide the window once that empty frame is drawn. Transport and like keep it open.
  import { onMount, tick } from "svelte";
  import { coverUrl, invoke, on } from "$lib/ipc";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import type { Track } from "$lib/ipc/bindings/Track";
  import type { TrackChanged } from "$lib/ipc/bindings/TrackChanged";
  import type { TrayAction } from "$lib/ipc/bindings/TrayAction";
  import type { TrayMenuSnapshot } from "$lib/ipc/bindings/TrayMenuSnapshot";
  import { strings } from "./strings";
  import Logo from "../../assets/logo.svg?raw";
  import IconPlay from "~icons/solar/play-bold";
  import IconPause from "~icons/solar/pause-bold";
  import IconPrev from "~icons/solar/skip-previous-bold";
  import IconNext from "~icons/solar/skip-next-bold";
  import IconHeart from "~icons/solar/heart-bold";
  import IconHeartLine from "~icons/solar/heart-linear";
  import IconMusic from "~icons/solar/music-note-2-linear";
  import IconShow from "~icons/solar/window-frame-linear";
  import IconHide from "~icons/solar/minimize-square-minimalistic-linear";
  import IconMini from "~icons/solar/pip-2-linear";
  import IconSettings from "~icons/solar/settings-linear";
  import IconQuit from "~icons/solar/power-linear";
  import IconCheck from "~icons/solar/unread-linear";

  let snap = $state<TrayMenuSnapshot>({ lang: "en", track: null, playing: false, windowShown: false, mini: false });
  /** The card is shown (and its entrance plays). */
  let open = $state(false);
  let card = $state<HTMLDivElement>();
  let coverFailed = $state(false);

  const t = $derived(strings(snap.lang));
  const track = $derived(snap.track);
  const artists = $derived(track?.artists.map((a) => a.name).join(", ") ?? "");
  const COVER = 44;
  const cover = $derived(coverUrl(track?.coverKey, COVER * (window.devicePixelRatio || 1) <= 72 ? 64 : 300));

  let opening = false;
  let shown = false;
  let unlisten: UnlistenFn[] = [];

  function apply(s: TrayMenuSnapshot) {
    if (s.track?.coverKey !== snap.track?.coverKey) coverFailed = false;
    snap = s;
  }

  async function show() {
    if (opening || shown) return;
    opening = true;
    try {
      apply(await invoke<TrayMenuSnapshot>("tray_menu_snapshot"));
      await tick();
      const r = card!.getBoundingClientRect();
      await listen();
      await invoke("tray_menu_reveal", { width: Math.ceil(r.width), height: Math.ceil(r.height) });
      shown = true;
      open = true;
      card?.focus({ preventScroll: true });
    } catch (e) {
      console.error("tray menu", e);
      stopListening();
    } finally {
      opening = false;
    }
  }

  /** Events matter only while the menu is on the screen. */
  async function listen() {
    stopListening();
    unlisten = await Promise.all([
      on<TrackChanged>("track:changed", (e) => {
        if (e.track.id !== snap.track?.id) coverFailed = false;
        snap.track = e.track;
      }),
      on<boolean>("tray-menu:playing", (p) => (snap.playing = p)),
      // A like elsewhere (or ours, confirmed) changes the track row.
      on("library:changed", () => void refresh()),
    ]);
  }

  function stopListening() {
    for (const f of unlisten) f();
    unlisten = [];
  }

  async function refresh() {
    const s = await invoke<TrayMenuSnapshot>("tray_menu_snapshot").catch(() => null);
    if (s && shown) apply(s);
  }

  function dismiss() {
    if (!shown) return;
    shown = false;
    open = false;
    stopListening();
    (document.activeElement as HTMLElement | null)?.blur?.();
    afterPaint(() => void invoke("tray_menu_hide").catch(() => {}));
  }

  /** Runs `fn` once the current DOM is on the screen (or shortly, if frames stall). */
  function afterPaint(fn: () => void) {
    let done = false;
    const run = () => {
      if (done) return;
      done = true;
      fn();
    };
    requestAnimationFrame(() => requestAnimationFrame(run));
    setTimeout(run, 120);
  }

  function act(action: TrayAction) {
    dismiss();
    void invoke("tray_menu_action", { action }).catch((e) => console.error("tray menu", e));
  }

  function toggle() {
    snap.playing = !snap.playing;
    void invoke("player_toggle").catch(() => {});
  }

  async function like(t: Track) {
    const liked = !t.liked;
    snap.track = { ...t, liked };
    try {
      await invoke("track_set_liked", { id: t.id, liked });
    } catch {
      if (snap.track?.id === t.id) snap.track = { ...snap.track, liked: !liked };
    }
  }

  // Keyboard: ↑/↓ between rows (the transport row counts as one), ←/→ along a row,
  // Enter/Space press the focused button, Esc closes, Home/End jump.
  function rows(): HTMLButtonElement[][] {
    if (!card) return [];
    return Array.from(card.querySelectorAll<HTMLElement>("[data-row]"))
      .map((r) => Array.from(r.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")))
      .filter((r) => r.length > 0);
  }

  function onKey(e: KeyboardEvent) {
    if (!shown) return;
    if (e.key === "Escape") {
      e.preventDefault();
      dismiss();
      return;
    }
    const all = rows();
    if (!all.length) return;
    const el = document.activeElement as HTMLButtonElement | null;
    let r = all.findIndex((row) => row.includes(el!));
    const c = r >= 0 ? all[r].indexOf(el!) : -1;
    // Each row's main button: the item itself, or play in the transport row.
    const main = (row: HTMLButtonElement[]) => row.find((b) => b.dataset.main !== undefined) ?? row[0];
    // With nothing focused, ↓ starts at the first window item and ↑ at the last.
    const firstItem = all.findIndex((row) => row[0].getAttribute("role") === "menuitem");
    let next: HTMLButtonElement | undefined;
    switch (e.key) {
      case "ArrowDown":
        next = main(all[r < 0 ? Math.max(0, firstItem) : (r + 1) % all.length]);
        break;
      case "ArrowUp":
        next = main(all[r < 0 ? all.length - 1 : (r - 1 + all.length) % all.length]);
        break;
      case "ArrowLeft":
      case "ArrowRight":
        if (r < 0) return;
        next = all[r][(c + (e.key === "ArrowLeft" ? -1 : 1) + all[r].length) % all[r].length];
        break;
      case "Home":
        next = main(all[0]);
        break;
      case "End":
        next = main(all[all.length - 1]);
        break;
      default:
        return;
    }
    e.preventDefault();
    next?.focus({ preventScroll: true });
  }

  /** One highlight at a time: the pointer takes it over from the keyboard. */
  function onPointerOver(e: PointerEvent) {
    const b = (e.target as HTMLElement).closest("button");
    const active = document.activeElement;
    if (b && active instanceof HTMLButtonElement && active !== b) active.blur();
  }

  onMount(() => {
    const un = on("tray-menu:open", () => void show());
    // Created for this very open: show as soon as the page and its font are ready (the
    // frame plugin's own page-load work is done by then too).
    const ready = document.readyState === "complete" ? Promise.resolve() : new Promise((r) => addEventListener("load", r, { once: true }));
    void ready.then(() => Promise.race([document.fonts.ready, new Promise((r) => setTimeout(r, 300))])).then(show);
    return () => {
      stopListening();
      void un.then((f) => f());
    };
  });

  const item =
    "flex w-full items-center gap-3 rounded-md px-2.5 py-2 text-left text-[13px] leading-5 text-ink outline-none transition-colors hover:bg-fill-2 focus-visible:bg-fill-2";
  const glyph = "grid place-items-center outline-none transition-colors";
</script>

<svelte:window onkeydown={onKey} onblur={dismiss} />

<div
  class="root"
  role="presentation"
  onpointerdown={(e) => {
    if (e.target === e.currentTarget) dismiss();
  }}
  oncontextmenu={(e) => e.preventDefault()}
>
  <!-- Opaque and unscaled: the entrance only fades and rises, so text stays sharp. -->
  <div
    bind:this={card}
    role="menu"
    aria-label={t.tray.menu}
    tabindex="-1"
    class="card w-[264px] rounded-[12px] bg-raised p-1.5 outline-none"
    class:open
    onpointerover={onPointerOver}
  >
    <div class="flex items-center gap-3 px-2 pt-1.5 pb-1" data-row>
      <div class="size-11 shrink-0 overflow-hidden rounded-md bg-fill-2">
        {#if !track}
          <div class="size-full">{@html Logo}</div>
        {:else if cover && !coverFailed}
          <img src={cover} alt="" width={COVER} height={COVER} draggable="false" class="size-full object-cover" onerror={() => (coverFailed = true)} />
        {:else}
          <div class="grid size-full place-items-center text-[16px] text-ink-3"><IconMusic /></div>
        {/if}
      </div>
      <div class="min-w-0 flex-1">
        <div class="truncate text-[13px] leading-[18px] font-medium text-ink">{track?.title ?? t.appName}</div>
        <div class="truncate text-[12px] leading-4 text-ink-3">{track ? artists : t.player.nothing}</div>
      </div>
      {#if track}
        <button
          type="button"
          aria-label={track.liked ? t.track.unlike : t.track.like}
          aria-pressed={track.liked}
          class="{glyph} size-8 shrink-0 rounded-md hover:bg-fill-2 focus-visible:bg-fill-2 {track.liked ? 'text-ink' : 'text-[#8a8a8e] hover:text-ink'}"
          onclick={() => like(track)}
        >
          {#if track.liked}<IconHeart class="size-[18px]" />{:else}<IconHeartLine class="size-[18px]" />{/if}
        </button>
      {/if}
    </div>

    <div class="flex items-center justify-center gap-6 pt-1 pb-2" data-row>
      <button
        type="button"
        aria-label={t.player.prev}
        disabled={!track}
        class="{glyph} size-8 rounded-full text-ink-2 hover:text-ink focus-visible:bg-fill-2 focus-visible:text-ink disabled:opacity-40"
        onclick={() => void invoke("player_prev").catch(() => {})}
      >
        <IconPrev class="size-[22px]" />
      </button>
      <button
        type="button"
        data-main
        aria-label={snap.playing ? t.player.pause : t.player.play}
        disabled={!track}
        class="{glyph} size-10 rounded-full bg-ink text-black hover:bg-[#dcdcdc] focus-visible:ring-2 focus-visible:ring-white/40 focus-visible:ring-offset-2 focus-visible:ring-offset-raised disabled:opacity-40"
        onclick={toggle}
      >
        {#if snap.playing}<IconPause class="size-5" />{:else}<IconPlay class="size-5" />{/if}
      </button>
      <button
        type="button"
        aria-label={t.player.next}
        disabled={!track}
        class="{glyph} size-8 rounded-full text-ink-2 hover:text-ink focus-visible:bg-fill-2 focus-visible:text-ink disabled:opacity-40"
        onclick={() => void invoke("player_next").catch(() => {})}
      >
        <IconNext class="size-[22px]" />
      </button>
    </div>

    <div class="mx-2 my-1 h-px bg-line"></div>

    <div data-row>
      <button type="button" role="menuitem" class={item} onclick={() => act("window")}>
        {#if snap.windowShown}<IconHide class="size-4 shrink-0 text-ink-2" />{:else}<IconShow class="size-4 shrink-0 text-ink-2" />{/if}
        <span class="flex-1 truncate">{snap.windowShown ? t.tray.hide : t.tray.show}</span>
      </button>
    </div>
    <div data-row>
      <button type="button" role="menuitemcheckbox" aria-checked={snap.mini} class={item} onclick={() => act("mini")}>
        <IconMini class="size-4 shrink-0 text-ink-2" />
        <span class="flex-1 truncate">{t.tray.miniPlayer}</span>
        {#if snap.mini}<IconCheck class="size-4 shrink-0 text-ink" />{/if}
      </button>
    </div>
    <div data-row>
      <button type="button" role="menuitem" class={item} onclick={() => act("settings")}>
        <IconSettings class="size-4 shrink-0 text-ink-2" />
        <span class="flex-1 truncate">{t.nav.settings}</span>
      </button>
    </div>

    <div class="mx-2 my-1 h-px bg-line"></div>

    <div data-row>
      <button type="button" role="menuitem" class={item} onclick={() => act("quit")}>
        <IconQuit class="size-4 shrink-0 text-ink-2" />
        <span class="flex-1 truncate">{t.tray.quit}</span>
      </button>
    </div>
  </div>
</div>
