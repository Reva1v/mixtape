// Typed access to the Rust core. Types come from ts-rs (`cargo test` regenerates them).
// Session commands go to the `mixtape` plugin, the app's own only when the host has them
// (route.ts).

import { invoke as tauriInvoke, Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppError } from "./bindings/AppError";
import { APP_COMMANDS, route } from "./route";

export { Channel };

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
  return listen<T>(event, (e) => handler(e.payload));
}

let coverSource: ((key: string, size: number) => string) | null = null;

/** Dev mock only (lib/ipc/mock.ts): covers drawn in the page instead of served by the core. */
export function setCoverSource(source: (key: string, size: number) => string) {
  coverSource = source;
}

/** URL of a cover thumbnail. Sizes are cut ahead of time: 64, 300 and 1024 px. */
export function coverUrl(key: string | null | undefined, size: 64 | 300 | 1024): string | undefined {
  if (!key) return undefined;
  return coverSource ? coverSource(key, size) : `http://cover.localhost/${key}/${size}`;
}
