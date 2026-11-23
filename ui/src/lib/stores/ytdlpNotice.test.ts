import { describe, expect, it } from "vitest";
import { shouldAnnounce } from "./ytdlpNotice";

function memory(initial?: string) {
  let value = initial ?? null;
  return {
    getItem: () => value,
    setItem: (_k: string, v: string) => {
      value = v;
    },
  };
}

describe("shouldAnnounce", () => {
  it("announces a version once", () => {
    const m = memory();
    expect(shouldAnnounce(m, "available", "2026.11.25")).toBe(true);
    expect(shouldAnnounce(m, "available", "2026.11.25")).toBe(false);
  });

  it("announces a newer version, and each kind on its own", () => {
    const m = memory();
    shouldAnnounce(m, "available", "2026.11.25");
    expect(shouldAnnounce(m, "available", "2026.12.01")).toBe(true);
    expect(shouldAnnounce(m, "installed", "2026.12.01")).toBe(true);
    expect(shouldAnnounce(m, "installed", "2026.12.01")).toBe(false);
    expect(shouldAnnounce(m, "available", "2026.12.01")).toBe(false);
  });

  it("survives broken storage contents", () => {
    for (const bad of ["not json", "[]", "null", "5", '{"available":5}']) {
      expect(shouldAnnounce(memory(bad), "available", "2026.11.25")).toBe(true);
    }
  });

  it("announces when the storage throws", () => {
    const broken = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    };
    expect(shouldAnnounce(broken, "available", "2026.11.25")).toBe(true);
  });
});
