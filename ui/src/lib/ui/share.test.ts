import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "$lib/ipc";
import { t } from "$lib/i18n";
import { toasts } from "$lib/stores/overlay.svelte";
import { isYtmPlaylist, isYtmTrack, share } from "./share";

vi.mock("$lib/ipc", () => ({ invoke: vi.fn() }));

describe("isYtmTrack", () => {
  it.each(["u1t22blUCnU", "dQw4w9WgXcQ", "-_-_-_-_-_-", "00000000000"])(
    "a YouTube Music video id: %s",
    (id) => {
      expect(isYtmTrack(id)).toBe(true);
    },
  );

  it.each([
    "local0000000001",
    "sc0123456789abcdef",
    "bc0123456789abcdef",
    "",
    "u1t22blUCn",
    "u1t22blUCnUx",
    "u1t22blUCn!",
    "u1t22blUCn ",
    "local:1234-abcd",
  ])("not shareable: %j", (id) => {
    expect(isYtmTrack(id)).toBe(false);
  });
});

describe("isYtmPlaylist", () => {
  it.each([
    "VLPLrAXtmErZgOeiKm4sgNOknGvNjby9efdf",
    "PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf",
    "OLAK5uy_abc",
  ])("a YouTube Music playlist: %s", (id) => {
    expect(isYtmPlaylist(id)).toBe(true);
  });

  it.each(["local:1234-abcd", "local:3f2b8c1e-0000-4000-8000-000000000000", "local:"])(
    "a local playlist: %s",
    (id) => {
      expect(isYtmPlaylist(id)).toBe(false);
    },
  );

  it("a playlist whose id merely contains `local` is still shareable", () => {
    expect(isYtmPlaylist("PLlocal:abc")).toBe(true);
    expect(isYtmPlaylist("localabc")).toBe(true);
  });
});

describe("share", () => {
  const writeText = vi.fn();

  beforeEach(() => {
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    toasts.list = [];
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
    toasts.list = [];
  });

  it("asks Rust for the link, copies it and says so", async () => {
    vi.mocked(invoke).mockResolvedValue("https://music.youtube.com/watch?v=u1t22blUCnU");
    writeText.mockResolvedValue(undefined);
    await share("track", "u1t22blUCnU");
    expect(invoke).toHaveBeenCalledWith("link_share_url", { kind: "track", id: "u1t22blUCnU" });
    expect(writeText).toHaveBeenCalledWith("https://music.youtube.com/watch?v=u1t22blUCnU");
    expect(toasts.list.map((x) => [x.text, x.level])).toEqual([[t.share.copied, "info"]]);
  });

  it("shows Rust's error message as an error toast and copies nothing", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("no link for this"));
    await share("playlist", "local:1");
    expect(invoke).toHaveBeenCalledWith("link_share_url", { kind: "playlist", id: "local:1" });
    expect(writeText).not.toHaveBeenCalled();
    expect(toasts.list.map((x) => [x.text, x.level])).toEqual([["no link for this", "error"]]);
  });

  it("falls back to the generic text when the error has no message, or the clipboard fails", async () => {
    vi.mocked(invoke).mockRejectedValue("boom");
    await share("album", "x");
    expect(toasts.list.map((x) => [x.text, x.level])).toEqual([[t.share.copyFailed, "error"]]);

    toasts.list = [];
    vi.mocked(invoke).mockResolvedValue("https://music.youtube.com/playlist?list=PLabc");
    writeText.mockRejectedValue(new Error(""));
    await share("playlist", "PLabc");
    expect(toasts.list.map((x) => [x.text, x.level])).toEqual([[t.share.copyFailed, "error"]]);
  });
});
