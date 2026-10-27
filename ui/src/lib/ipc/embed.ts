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

export const embedded =
  typeof window !== "undefined" &&
  islandFrame() &&
  // A plugin's init script may leave a bare `__TAURI_INTERNALS__ = { plugins }` in the frame;
  // only a real one (with `invoke`) means Tauri's own IPC is here.
  typeof (window as { __TAURI_INTERNALS__?: { invoke?: unknown } }).__TAURI_INTERNALS__?.invoke !== "function";

function post(msg: FromFrame) {
  window.parent.postMessage(msg, "*");
}

/** Tell the island something the frame cannot do itself. */
export function toIsland(t: "close" | "standalone") {
  if (embedded) post({ mt: 1, t });
}

/** The shim itself; exported for the tests (`install(fakeWindow, send)`). */
export function install(
  w: Window & typeof globalThis,
  send: (m: FromFrame) => void,
): (m: ToFrame) => void {
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
  Object.defineProperty(w, "__TAURI_INTERNALS__", { value: internals, configurable: true });
  // `listen`'s unlisten calls this before `plugin:event|unlisten`; the island drops its
  // listener on that command.
  Object.defineProperty(w, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
    value: { unregisterListener() {} },
    configurable: true,
  });

  return (m: ToFrame) => {
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
}

/** The field that had the keys, for `refocus`. */
let lastField: HTMLElement | null = null;
/** Fields that take typing (not checkboxes or sliders). */
const FIELD =
  'input:not([type="checkbox"]):not([type="radio"]):not([type="range"]):not([type="button"]), textarea, [contenteditable="true"]';
const isField = (el: EventTarget | null): el is HTMLElement =>
  el instanceof HTMLElement && el.matches(FIELD);

if (embedded) {
  document.documentElement.classList.add("in-island");
  const receive = install(window, post);
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
