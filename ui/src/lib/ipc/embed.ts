// Mixtape inside Droplet Island: the island's page shows this interface in a frame
// (`?host=island`), and a frame gets none of Tauri's scripts. This file stands in for them: a
// small `__TAURI_INTERNALS__` that hands every call to the island's page by `postMessage`; the
// page makes the call and sends back the answer, channel messages and events
// (droplet-island `ui/src/lib/music/embed-bridge.ts` speaks the other end). Imported first by
// main.ts, so it is in place before anything calls Tauri. Outside the island it does nothing:
// the Mixtape app looks and works exactly as before.

/** Every message either way carries `mt: 1`. */
export type FromFrame =
  | { mt: 1; t: "invoke"; id: number; cmd: string; args: unknown }
  | { mt: 1; t: "drop"; id: number }
  | { mt: 1; t: "keys"; on: boolean }
  | { mt: 1; t: "close" }
  | { mt: 1; t: "standalone" };
export type ToFrame =
  | { mt: 1; t: "result"; id: number; ok: boolean; value: unknown }
  | { mt: 1; t: "cb"; id: number; raw: unknown }
  | { mt: 1; t: "refocus" };

/** The island shows this page in a frame. */
export function islandFrame(w: Window = window): boolean {
  try {
    return w.parent !== w && new URLSearchParams(w.location.search).get("host") === "island";
  } catch {
    return false;
  }
}

/** Shown inside the island: a compact player bar, the logo opens the app, a way back. */
export const embedded = typeof window !== "undefined" && islandFrame();

/** Inside the island every call goes through the island's page. WebView2 may inject Tauri's
 * full `__TAURI_INTERNALS__` into the frame (non-configurable), but Tauri delivers channel
 * messages and event callbacks to the top frame only, so `$lib/ipc` uses [`bridge`]
 * instead of it. The dev mock (`?mock`) brings its own IPC. */
export function bridgeFor(isEmbedded: boolean, search: string): boolean {
  return isEmbedded && !new URLSearchParams(search).has("mock");
}

function post(msg: FromFrame) {
  window.parent.postMessage(msg, "*");
}

/** Tell the island something the frame cannot do itself. */
export function toIsland(t: "close" | "standalone") {
  if (embedded) post({ mt: 1, t });
}

/** What the frame's IPC needs from the island: the subset of Tauri's internals that
 * `invoke`, channels and events use. */
export type Bridge = {
  transformCallback(cb?: (raw: unknown) => void, once?: boolean): number;
  unregisterCallback(id: number): void;
  invoke(cmd: string, args?: unknown): Promise<unknown>;
};

/** The bridge to the island's page: calls go out by `send`, answers come back through the
 * returned `receive`. Exported for the tests. */
export function createBridge(send: (m: FromFrame) => void): {
  internals: Bridge & Record<string, unknown>;
  receive: (m: ToFrame) => void;
} {
  let next = 1;
  const callbacks = new Map<number, (raw: unknown) => void>();
  const pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: unknown) => void }
  >();

  const internals = {
    transformCallback(cb?: (raw: unknown) => void, once = false): number {
      const id = next++;
      callbacks.set(id, (raw) => {
        if (once) callbacks.delete(id);
        cb?.(raw);
      });
      return id;
    },
    unregisterCallback(id: number) {
      callbacks.delete(id);
      send({ mt: 1, t: "drop", id });
    },
    invoke(cmd: string, args: unknown = {}): Promise<unknown> {
      const id = next++;
      // Channels become their `__CHANNEL__:<id>` strings here, as Tauri's own IPC sends them.
      const plain = args === undefined ? {} : JSON.parse(JSON.stringify(args));
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        send({ mt: 1, t: "invoke", id, cmd, args: plain });
      });
    },
    convertFileSrc(path: string, protocol = "asset") {
      return `http://${protocol}.localhost/${encodeURIComponent(path)}`;
    },
    metadata: {
      currentWindow: { label: "island" },
      currentWebview: { windowLabel: "island", label: "island" },
    },
  };
  const receive = (m: ToFrame) => {
    if (m.t === "result") {
      const p = pending.get(m.id);
      if (!p) return;
      pending.delete(m.id);
      if (m.ok) p.resolve(m.value);
      else p.reject(m.value);
    } else if (m.t === "cb") {
      callbacks.get(m.id)?.(m.raw);
    }
  };
  return { internals, receive };
}

/** Also puts the bridge in place of `window.__TAURI_INTERNALS__`, for Tauri's own API calls
 * (window, webview). Only where Tauri left the frame without a usable `invoke`: some
 * WebView2 builds inject a full, non-configurable object, which cannot and need not be
 * replaced (the app's own IPC goes through [`bridge`] anyway). Never throws. */
export function install(w: Window & typeof globalThis, send: (m: FromFrame) => void): (m: ToFrame) => void {
  const { internals, receive } = createBridge(send);
  const current = (w as { __TAURI_INTERNALS__?: { invoke?: unknown } }).__TAURI_INTERNALS__;
  if (typeof current?.invoke !== "function") {
    try {
      // Not writable, so a later plain assignment can't swap Tauri's back in.
      Object.defineProperty(w, "__TAURI_INTERNALS__", { value: internals, configurable: true });
      // `listen`'s unlisten calls this before `plugin:event|unlisten`.
      Object.defineProperty(w, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
        value: { unregisterListener() {} },
        configurable: true,
      });
    } catch (e) {
      console.warn("island: could not replace Tauri's internals", e);
    }
  }
  bridge = internals;
  return receive;
}

/** Tauri's `listen` over the island's bridge (`plugin:event|listen` / `unlisten`, as the
 * island's embed bridge expects). Exported for the tests. */
export async function bridgeListen<T>(via: Bridge, event: string, handler: (payload: T) => void): Promise<() => void> {
  const cb = via.transformCallback((raw) => handler((raw as { payload: T }).payload));
  const eventId = (await via.invoke("plugin:event|listen", { event, target: { kind: "Any" }, handler: cb })) as number;
  return () => {
    via.unregisterCallback(cb);
    void via.invoke("plugin:event|unlisten", { event, eventId }).catch(() => {});
  };
}

/** The island's bridge, when this page runs inside the island: `$lib/ipc` sends invokes,
 * channels and event listeners through it instead of Tauri's IPC, which in a frame doesn't
 * deliver channel messages or events. */
export let bridge: Bridge | null = null;

/** The field that had the keys, for `refocus`. */
let lastField: HTMLElement | null = null;

/** Fields that take typing (not checkboxes or sliders). */
const FIELD =
  'input:not([type="checkbox"]):not([type="radio"]):not([type="range"]):not([type="button"]), textarea, [contenteditable="true"]';
const isField = (el: EventTarget | null): el is HTMLElement =>
  el instanceof HTMLElement && el.matches(FIELD);

if (embedded) {
  document.documentElement.classList.add("in-island");
  let receive: (m: ToFrame) => void = () => {};
  try {
    if (bridgeFor(embedded, location.search)) receive = install(window, post);
  } catch (e) {
    // A broken bridge must never blank the page.
    console.error("island: bridge failed", e);
  }
  window.addEventListener("message", (e) => {
    if (e.source !== window.parent) return;
    const m = e.data as ToFrame | null;
    if (!m || m.mt !== 1) return;
    if (m.t === "refocus") lastField?.focus();
    else receive(m);
  });
  // A text field wants the keyboard: the island takes it (its window never does on its own).
  window.addEventListener("focusin", (e) => {
    if (!isField(e.target)) return;
    lastField = e.target;
    post({ mt: 1, t: "keys", on: true });
  });
  // The island's window has no focus of its own, so a click may not move it: ask on the press too.
  window.addEventListener("pointerdown", (e) => {
    const el = e.target instanceof Element ? e.target.closest(FIELD) : null;
    if (!(el instanceof HTMLElement)) return;
    lastField = el;
    post({ mt: 1, t: "keys", on: true });
  });
  window.addEventListener("focusout", (e) => {
    if (isField(e.target) && !isField(e.relatedTarget)) post({ mt: 1, t: "keys", on: false });
  });
  // Esc that nothing here took (a menu, a dialog, the queue) leaves Mixtape for the island's Home.
  window.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || e.isComposing) return;
    setTimeout(() => !e.defaultPrevented && post({ mt: 1, t: "close" }));
  });
}
