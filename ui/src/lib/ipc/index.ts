// Typed access to the Rust core. Types come from ts-rs (`cargo test` regenerates them).
// Session commands go to the `mixtape` plugin, the app's own only when the host has them
// (route.ts).

import { invoke as tauriCoreInvoke, Channel as TauriChannel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppError } from "./bindings/AppError";
import { APP_COMMANDS, route } from "./route";
import { bridge, bridgeListen, type Bridge } from "./embed";
import { coverReady } from "$lib/stores/coverReady.svelte";

/** Tauri's invoke, or the island's bridge inside the island. */
function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return bridge ? (bridge.invoke(cmd, args) as Promise<T>) : tauriCoreInvoke<T>(cmd, args);
}

/** Tauri's `Channel` over the island's bridge: same wire form (`__CHANNEL__:<id>`), same
 * in-order delivery of `{ index, message }` and `{ index, end }`. Exported for the tests. */
export class BridgeChannel<T> {
  readonly id: number;
  onmessage: (message: T) => void;
  #next = 0;
  #pending = new Map<number, T>();
  #end: number | undefined;

  constructor(readonly via: Bridge, onmessage?: (message: T) => void) {
    this.onmessage = onmessage ?? (() => {});
    this.id = via.transformCallback((raw) => this.#take(raw as { index: number; message?: T; end?: true }));
  }

  #take(raw: { index: number; message?: T; end?: true }) {
    if (raw.end) {
      if (raw.index === this.#next) this.cleanupCallback();
      else this.#end = raw.index;
      return;
    }
    if (raw.index !== this.#next) {
      this.#pending.set(raw.index, raw.message as T);
      return;
    }
    this.#deliver(raw.message as T);
    while (this.#pending.has(this.#next)) {
      const m = this.#pending.get(this.#next) as T;
      this.#pending.delete(this.#next);
      this.#deliver(m);
    }
    if (this.#next === this.#end) this.cleanupCallback();
  }

  #deliver(m: T) {
    this.#next++;
    try {
      this.onmessage(m);
    } catch (e) {
      queueMicrotask(() => {
        throw e;
      });
    }
  }

  cleanupCallback() {
    this.via.unregisterCallback(this.id);
  }

  toJSON() {
    return `__CHANNEL__:${this.id}`;
  }
}

/** `new Channel<T>()` as with Tauri's; inside the island it rides the bridge. */
export const Channel = function <T>(this: unknown, onmessage?: (message: T) => void) {
  return bridge ? new BridgeChannel<T>(bridge, onmessage) : new TauriChannel<T>(onmessage);
} as unknown as typeof TauriChannel;
export type Channel<T> = TauriChannel<T>;

export class IpcError extends Error {
  constructor(readonly kind: AppError["kind"], message: string) {
    super(message);
  }
}

let caps: Promise<Set<string> | null> | null = null;

/** Whether the host app offers `cmd` (one of `APP_COMMANDS`): the Mixtape app has them
 * all, the island none. Asked once; when the answer is unknown, assumed so. */
export async function hostHas(cmd: string): Promise<boolean> {
  caps ??= tauriInvoke<string[]>(route("host_caps")).then(
    (list) => new Set(list),
    () => null,
  );
  const set = await caps;
  return set ? set.has(cmd) : true;
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (APP_COMMANDS.has(cmd) && !(await hostHas(cmd))) {
    throw new IpcError("notFound", `${cmd} is not available here`);
  }
  try {
    return await tauriInvoke<T>(route(cmd), args);
  } catch (e) {
    if (e && typeof e === "object" && "kind" in e && "message" in e) {
      const err = e as AppError;
      throw new IpcError(err.kind, err.message);
    }
    throw new IpcError("internal", String(e));
  }
}

export function on<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  return bridge ? bridgeListen(bridge, event, handler) : listen<T>(event, (e) => handler(e.payload));
}


let coverSource: ((key: string, size: number) => string) | null = null;

/** Dev mock only (lib/ipc/mock.ts): covers drawn in the page instead of served by the core. */
export function setCoverSource(source: (key: string, size: number) => string) {
  coverSource = source;
}

/** URL of a cover thumbnail. Sizes are cut ahead of time: 64, 300 and 1024 px. */
export function coverUrl(key: string | null | undefined, size: 64 | 300 | 1024): string | undefined {
  if (!key) return undefined;
  if (coverSource) return coverSource(key, size);
  // A cover that landed after a failed load is asked for again under a new version.
  const v = coverReady.version(key);
  return `http://cover.localhost/${key}/${size}${v ? `?v=${v}` : ""}`;
}
