import { describe, expect, it } from "vitest";
import { isYtmPlaylist, isYtmTrack } from "./share";

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
  });
});
