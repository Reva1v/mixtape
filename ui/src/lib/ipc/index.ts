// Typed access to the Rust core. Types come from ts-rs (`cargo test` regenerates them).

import { invoke as tauriInvoke, Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppError } from "./bindings/AppError";

export { Channel };

export class IpcError extends Error {
  constructor(readonly kind: AppError["kind"], message: string) {
    super(message);
  }
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args);
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
