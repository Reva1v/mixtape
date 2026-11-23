// "Announce once" for the yt-dlp toasts, apart from the store so it can be tested: what was
// last announced per kind is kept, so the same version never toasts twice across restarts.

export type NoticeKind = "available" | "installed" | "rolledback";

const KEY = "mixtape.ytdlp.announced";

type Memory = Pick<Storage, "getItem" | "setItem">;

function read(store: Memory): Partial<Record<NoticeKind, string>> {
  try {
    const v: unknown = JSON.parse(store.getItem(KEY) ?? "{}");
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Partial<Record<NoticeKind, string>>) : {};
  } catch {
    return {};
  }
}

/** True the first time `version` is announced for `kind`; remembers it. A storage that
 * cannot be read or written announces (better a repeat than silence). */
export function shouldAnnounce(store: Memory, kind: NoticeKind, version: string): boolean {
  const seen = read(store);
  if (seen[kind] === version) return false;
  try {
    store.setItem(KEY, JSON.stringify({ ...seen, [kind]: version }));
  } catch {
    /* announce anyway */
  }
  return true;
}
