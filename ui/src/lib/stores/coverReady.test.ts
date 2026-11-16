import { describe, expect, it } from "vitest";
import { createCoverReady } from "./coverReady.svelte";

describe("coverReady", () => {
  it("starts at version 0 and ignores a missing key", () => {
    const c = createCoverReady();
    expect(c.version("a")).toBe(0);
    expect(c.version(null)).toBe(0);
    expect(c.version(undefined)).toBe(0);
    c.failed(null);
    c.failed(undefined);
    c.ready([]);
    expect(c.version("a")).toBe(0);
  });

  it("asks again when the cover is announced after its load failed", () => {
    const c = createCoverReady();
    c.failed("a");
    expect(c.version("a")).toBe(0);
    c.ready(["a", "b"]);
    expect(c.version("a")).toBe(1);
    expect(c.version("b")).toBe(0);
  });

  it("asks again at once when the announcement came before the failure", () => {
    const c = createCoverReady();
    c.ready(["a"]);
    expect(c.version("a")).toBe(0);
    c.failed("a");
    expect(c.version("a")).toBe(1);
  });

  it("does not loop on a cover that is really absent", () => {
    const c = createCoverReady();
    c.ready(["a"]);
    c.failed("a"); // retry 1
    c.failed("a"); // fails again: waits for another announcement
    c.failed("a");
    expect(c.version("a")).toBe(1);
    c.ready(["a"]);
    expect(c.version("a")).toBe(2);
  });

  it("does not retry a cover that loaded fine", () => {
    const c = createCoverReady();
    c.ready(["a"]);
    expect(c.version("a")).toBe(0);
  });

  it("keeps versions per key", () => {
    const c = createCoverReady();
    c.failed("a");
    c.failed("b");
    c.ready(["b"]);
    expect(c.version("a")).toBe(0);
    expect(c.version("b")).toBe(1);
  });

  it("remembers only the latest keys it waits on", () => {
    const c = createCoverReady(2);
    c.failed("a");
    c.failed("b");
    c.failed("c"); // pushes "a" out
    c.ready(["a", "b", "c"]);
    expect(c.version("a")).toBe(0);
    expect(c.version("b")).toBe(1);
    expect(c.version("c")).toBe(1);
  });
});
