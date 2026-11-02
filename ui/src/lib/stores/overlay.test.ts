import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toasts } from "./overlay.svelte";

describe("toasts", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    toasts.list.map((t) => t.id).forEach((id) => toasts.dismiss(id));
    vi.useRealTimers();
  });

  it("shows the same message once however often it comes", () => {
    for (let i = 0; i < 50; i++) toasts.push("expired", "error", 10_000);
    expect(toasts.list).toHaveLength(1);
  });

  it("keeps a repeated message up for its full time from the last repeat", () => {
    toasts.push("expired", "error", 10_000);
    vi.advanceTimersByTime(9_000);
    toasts.push("expired", "error", 10_000);
    vi.advanceTimersByTime(9_000);
    expect(toasts.list).toHaveLength(1);
    vi.advanceTimersByTime(1_000);
    expect(toasts.list).toHaveLength(0);
  });

  it("keeps different messages apart, at most three, dropping the oldest", () => {
    ["a", "b", "c", "d"].forEach((text) => toasts.push(text));
    expect(toasts.list.map((t) => t.text)).toEqual(["b", "c", "d"]);
  });

  it("the same text at another level is another toast", () => {
    toasts.push("same", "info");
    toasts.push("same", "error");
    expect(toasts.list).toHaveLength(2);
  });
});
