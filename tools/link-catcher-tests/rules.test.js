import test from "node:test";
import assert from "node:assert/strict";
import { catchable, fromOutside, handoffUrl } from "../link-catcher/rules.js";

const off = { catchYoutube: false };
const on = { catchYoutube: true };

test("every YouTube Music content form is caught", () => {
  for (const url of [
    "https://music.youtube.com/watch?v=abc123&list=RDAMVM",
    "https://music.youtube.com/playlist?list=PLabc",
    "https://music.youtube.com/browse/MPREb_abc123",
    "https://music.youtube.com/browse/VLPLabc",
    "https://music.youtube.com/channel/UCabc123",
  ]) {
    assert.equal(catchable(url, off), true, url);
    assert.equal(catchable(url, on), true, url);
  }
});

test("YouTube Music non-content pages are not caught", () => {
  for (const url of [
    "https://music.youtube.com/",
    "https://music.youtube.com/library",
    "https://music.youtube.com/explore",
    "https://music.youtube.com/watch",
    "https://music.youtube.com/playlist",
    "https://music.youtube.com/browse/FEmusic_home",
  ]) {
    assert.equal(catchable(url, on), false, url);
  }
});

test("plain YouTube is caught only when the setting is on", () => {
  for (const url of [
    "https://www.youtube.com/watch?v=abc",
    "https://m.youtube.com/watch?v=abc",
    "https://youtube.com/watch?v=abc",
    "https://www.youtube.com/shorts/abc",
    "https://youtu.be/abc",
  ]) {
    assert.equal(catchable(url, off), false, `off: ${url}`);
    assert.equal(catchable(url, undefined), false, `default: ${url}`);
    assert.equal(catchable(url, on), true, `on: ${url}`);
  }
  assert.equal(catchable("https://www.youtube.com/", on), false);
  assert.equal(catchable("https://www.youtube.com/feed/subscriptions", on), false);
  assert.equal(catchable("https://youtu.be/", on), false);
});

test("other sites and junk are never caught", () => {
  for (const url of ["https://example.com/watch?v=abc", "not a url", "", "chrome://newtab/", "ftp://music.youtube.com/watch?v=a"]) {
    assert.equal(catchable(url, on), false, url);
  }
});

test("fromOutside: a new tab or typed URL counts as outside", () => {
  for (const cur of [undefined, null, "", "about:blank", "chrome://newtab/", "edge://newtab/", "https://example.com/page", "https://www.google.com/search?q=x"]) {
    assert.equal(fromOutside(cur), true, String(cur));
  }
});

test("fromOutside: browsing inside YouTube is not outside", () => {
  for (const cur of ["https://music.youtube.com/", "https://music.youtube.com/watch?v=a", "https://www.youtube.com/", "https://youtu.be/a"]) {
    assert.equal(fromOutside(cur), false, cur);
  }
});

test("handoffUrl percent-encodes the original URL", () => {
  const u = handoffUrl("https://music.youtube.com/watch?v=a&list=b");
  assert.equal(u, "mixtape://open?url=https%3A%2F%2Fmusic.youtube.com%2Fwatch%3Fv%3Da%26list%3Db");
  assert.ok(!u.slice("mixtape://open?url=".length).includes("&"));
  assert.ok(!u.slice("mixtape://open?url=".length).includes("?"));
});
