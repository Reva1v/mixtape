import { describe, expect, it } from "vitest";
import { mayBeLink } from "./linkHint";

describe("mayBeLink", () => {
  it.each([
    "https://music.youtube.com/watch?v=u1t22blUCnU&si=abc",
    "https://www.youtube.com/watch?v=u1t22blUCnU",
    "https://m.youtube.com/watch?v=u1t22blUCnU",
    "https://youtu.be/u1t22blUCnU?t=5",
    "https://www.youtube.com/shorts/u1t22blUCnU",
    "music.youtube.com/playlist?list=PLabc",
    "HTTPS://MUSIC.YOUTUBE.COM/browse/MPREb_x",
    "listen to this https://youtu.be/u1t22blUCnU please",
  ])("asks Rust about a link with a YouTube host: %s", (text) => {
    expect(mayBeLink(text)).toBe(true);
  });

  it.each([
    "u1t22blUCnU",
    "  u1t22blUCnU  ",
    "PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf",
    "MPREb_abcdefghij",
    "UCuAXFkgsw1L7xaCfnd5JJOw",
  ])("asks Rust about a bare id that is the whole text: %s", (text) => {
    expect(mayBeLink(text)).toBe(true);
  });

  it.each([
    "",
    "   ",
    "radiohead",
    "daft punk",
    "the weeknd blinding lights",
    "AC/DC",
    "u1t22blUCn",
    "hello, u1t22blUCnU world",
  ])("skips ordinary text: %j", (text) => {
    expect(mayBeLink(text)).toBe(false);
  });

  it("skips a long single token that cannot be an id", () => {
    expect(mayBeLink("a".repeat(65))).toBe(false);
  });
});
