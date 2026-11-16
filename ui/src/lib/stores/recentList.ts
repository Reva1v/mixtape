// The list logic of the recent searches, apart from the store so it can be tested. It mirrors
// what the core does (crates/library/src/recent.rs): the same key, so the optimistic list the
// interface shows first is the one the core then confirms.

export type RecentItem =
  | { kind: "query"; text: string }
  | {
      kind: "track" | "album" | "artist" | "playlist";
      id: string;
      title: string;
      /** Artists; the kind label is added when shown, in the current language. */
      subtitle?: string;
      coverKey?: string | null;
    };

/** Entries kept, as in the core. */
export const MAX = 20;

/** A query's text with single spaces and no padding. */
const tidyText = (text: string) => text.trim().split(/\s+/).filter(Boolean).join(" ");

/** What two entries are the same by: a query by its tidy, lower-case text, the rest by kind and id. */
export function keyOf(item: RecentItem): string {
  return item.kind === "query" ? `q:${tidyText(item.text).toLowerCase()}` : `${item.kind}:${item.id}`;
}

/** The list with `item` on top (replacing the same entry), or the list itself when `item` is empty. */
export function added(list: RecentItem[], item: RecentItem): RecentItem[] {
  const entry: RecentItem = item.kind === "query" ? { kind: "query", text: tidyText(item.text) } : item;
  if (entry.kind === "query" ? !entry.text : !entry.id) return list;
  const key = keyOf(entry);
  return [entry, ...list.filter((x) => keyOf(x) !== key)].slice(0, MAX);
}

/** The list without `item`. */
export function removed(list: RecentItem[], item: RecentItem): RecentItem[] {
  const key = keyOf(item);
  return list.filter((x) => keyOf(x) !== key);
}

const isText = (v: unknown): v is string => typeof v === "string";

/** The entries of the list this interface used to keep in the browser (a JSON array), newest
 * first, without anything that is not an entry. */
export function readLegacy(raw: string | null): RecentItem[] {
  let list: unknown;
  try {
    list = raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
  if (!Array.isArray(list)) return [];
  const out: RecentItem[] = [];
  for (const x of list) {
    if (!x || typeof x !== "object") continue;
    const o = x as Record<string, unknown>;
    if (o.kind === "query" && isText(o.text) && tidyText(o.text)) {
      out.push({ kind: "query", text: o.text });
    } else if ((o.kind === "track" || o.kind === "album" || o.kind === "artist" || o.kind === "playlist") && isText(o.id) && o.id && isText(o.title)) {
      out.push({
        kind: o.kind,
        id: o.id,
        title: o.title,
        ...(isText(o.subtitle) ? { subtitle: o.subtitle } : {}),
        ...(isText(o.coverKey) || o.coverKey === null ? { coverKey: o.coverKey as string | null } : {}),
      });
    }
  }
  return out.slice(0, MAX);
}
