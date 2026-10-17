// What the user looked for and opened from search, shown when the search box is empty.
// Kept in this browser's storage: a per-user convenience, fine to lose.

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

// v2: subtitles no longer carry the (language-bound) kind label.
const KEY = "mixtape.recentSearch.v2";
const MAX = 12;

function load(): RecentItem[] {
  try {
    const raw = localStorage.getItem(KEY);
    const list = raw ? (JSON.parse(raw) as RecentItem[]) : [];
    return Array.isArray(list) ? list.slice(0, MAX) : [];
  } catch {
    return [];
  }
}

const same = (a: RecentItem, b: RecentItem) =>
  a.kind === b.kind && (a.kind === "query" ? a.text === (b as typeof a).text : a.id === (b as { id: string }).id);

class RecentSearch {
  items = $state<RecentItem[]>(load());

  add(item: RecentItem) {
    if (item.kind === "query" && !item.text.trim()) return;
    this.items = [item, ...this.items.filter((x) => !same(x, item))].slice(0, MAX);
    this.#save();
  }

  remove(item: RecentItem) {
    this.items = this.items.filter((x) => !same(x, item));
    this.#save();
  }

  clear() {
    this.items = [];
    this.#save();
  }

  #save() {
    try {
      localStorage.setItem(KEY, JSON.stringify(this.items));
    } catch {
      // Storage unavailable: the list just lives for this session.
    }
  }
}

export const recentSearch = new RecentSearch();
