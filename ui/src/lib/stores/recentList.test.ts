import { describe, expect, it } from "vitest";
import { MAX, added, keyOf, readLegacy, removed, type RecentItem } from "./recentList";

const q = (text: string): RecentItem => ({ kind: "query", text });
const track = (id: string, title = id): RecentItem => ({ kind: "track", id, title });
const texts = (list: RecentItem[]) => list.map((i) => (i.kind === "query" ? i.text : i.id));

describe("keyOf", () => {
  it("normalises a query's text but keeps kind and id apart", () => {
    expect(keyOf(q("  Loser "))).toBe("q:loser");
    expect(keyOf(q("a   B\tc"))).toBe("q:a b c");
    expect(keyOf(q("ПРИВІТ"))).toBe(keyOf(q("привіт ")));
    expect(keyOf(track("1"))).toBe("track:1");
    expect(keyOf({ kind: "album", id: "1", title: "x" })).toBe("album:1");
  });
});

describe("added", () => {
  it("puts the newest on top and dedupes by the key", () => {
    let list: RecentItem[] = [];
    list = added(list, q("Loser"));
    list = added(list, track("t1"));
    list = added(list, q("  loser "));
    expect(texts(list)).toEqual(["loser", "t1"]);
  });

  it("ignores an empty query or id and tidies spaces", () => {
    const list = [q("a")];
    expect(added(list, q("   "))).toBe(list);
    expect(added(list, { kind: "track", id: "", title: "x" })).toBe(list);
    expect(texts(added([], q("  two   words ")))).toEqual(["two words"]);
  });

  it("keeps the newest twenty", () => {
    let list: RecentItem[] = [];
    for (let i = 0; i < 25; i++) list = added(list, q(`query ${i}`));
    expect(list).toHaveLength(MAX);
    expect(texts(list)[0]).toBe("query 24");
    expect(texts(list)[MAX - 1]).toBe("query 5");
  });

  it("does not touch the list it was given", () => {
    const list = [q("a")];
    added(list, q("b"));
    expect(list).toEqual([q("a")]);
  });
});

describe("removed", () => {
  it("drops the entry with the same key", () => {
    expect(texts(removed([q("A"), q("b")], q(" a ")))).toEqual(["b"]);
    expect(removed([q("a")], q("zzz"))).toEqual([q("a")]);
  });
});

describe("readLegacy", () => {
  it("reads the browser list of the old format, newest first", () => {
    const raw = JSON.stringify([q("Loser"), { kind: "album", id: "m1", title: "Odelay", subtitle: "Beck", coverKey: null }, track("t1")]);
    const list = readLegacy(raw);
    expect(texts(list)).toEqual(["Loser", "m1", "t1"]);
    expect(list[1]).toEqual({ kind: "album", id: "m1", title: "Odelay", subtitle: "Beck", coverKey: null });
  });

  it("survives nothing, junk and half-valid lists", () => {
    expect(readLegacy(null)).toEqual([]);
    expect(readLegacy("not json")).toEqual([]);
    expect(readLegacy('{"a":1}')).toEqual([]);
    const mixed = JSON.stringify([null, 5, { kind: "query", text: "  " }, { kind: "track" }, { kind: "weird", id: "x", title: "y" }, q("ok")]);
    expect(readLegacy(mixed)).toEqual([q("ok")]);
  });

  it("caps an oversized list", () => {
    const raw = JSON.stringify(Array.from({ length: 50 }, (_, i) => q(`q${i}`)));
    expect(readLegacy(raw)).toHaveLength(MAX);
  });
});
