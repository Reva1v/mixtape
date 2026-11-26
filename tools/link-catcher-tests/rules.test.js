import test from "node:test";
import assert from "node:assert/strict";
import { catchable, caughtCommit, handoffExit, handoffUrl, youtubeUrl } from "../link-catcher/rules.js";

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
    "https://music.youtube.com/browse/MPREb_",
    "https://music.youtube.com/browse/VL",
    "https://music.youtube.com/channel/abc123",
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

test("a plain http link is caught like https", () => {
  assert.equal(catchable("http://music.youtube.com/watch?v=abc", off), true);
  assert.equal(catchable("http://youtu.be/abc", on), true);
});

test("other sites and junk are never caught", () => {
  for (const url of ["https://example.com/watch?v=abc", "not a url", "", "chrome://newtab/", "ftp://music.youtube.com/watch?v=a"]) {
    assert.equal(catchable(url, on), false, url);
  }
});

test("caughtCommit: a link from another app, a typed address or a bookmark is caught", () => {
  for (const transitionType of ["auto_toplevel", "typed", "generated", "keyword", "keyword_generated", "auto_bookmark", "start_page"]) {
    assert.equal(caughtCommit({ transitionType }), true, transitionType);
    // even when the tab was on YouTube before: the user asked for this address
    assert.equal(caughtCommit({ transitionType, prevWasYoutube: true }), true, transitionType + " after YouTube");
  }
});

test("caughtCommit: a link is caught only when the tab wasn't on YouTube", () => {
  assert.equal(caughtCommit({ transitionType: "link" }), true);
  assert.equal(caughtCommit({ transitionType: "link", prevWasYoutube: false }), true);
  assert.equal(caughtCommit({ transitionType: "link", prevWasYoutube: true }), false);
  assert.equal(caughtCommit({ transitionType: "link", transitionQualifiers: ["client_redirect"], prevWasYoutube: true }), false);
});

test("caughtCommit: back/forward, reload, forms and frames are never caught", () => {
  assert.equal(caughtCommit({ transitionType: "link", transitionQualifiers: ["forward_back"] }), false);
  assert.equal(caughtCommit({ transitionType: "auto_toplevel", transitionQualifiers: ["forward_back"] }), false);
  assert.equal(caughtCommit({ transitionType: "typed", transitionQualifiers: ["forward_back", "from_address_bar"] }), false);
  for (const transitionType of ["reload", "form_submit", "auto_subframe", "manual_subframe", "", undefined]) {
    assert.equal(caughtCommit({ transitionType }), false, String(transitionType));
  }
});

test("caughtCommit: the from_address_bar qualifier alone doesn't block a typed address", () => {
  assert.equal(caughtCommit({ transitionType: "typed", transitionQualifiers: ["from_address_bar"] }), true);
});

test("youtubeUrl: YouTube hosts only; hidden or other URLs are not", () => {
  for (const u of ["https://music.youtube.com/", "https://www.youtube.com/watch?v=a", "https://youtube.com/", "https://youtu.be/a", "https://m.youtube.com/"]) {
    assert.equal(youtubeUrl(u), true, u);
  }
  for (const u of [undefined, null, "", "about:blank", "edge://newtab/", "chrome-extension://njfkgiakgfoadoknhlokkcjapjpanfga/handoff.html", "https://notyoutube.com/", "https://youtube.com.evil.com/", "not a url"]) {
    assert.equal(youtubeUrl(u), false, String(u));
  }
});

test("handoffExit: steps past the caught page, or closes a tab that had nothing before it", () => {
  assert.deepEqual(handoffExit(1), { close: true });
  assert.deepEqual(handoffExit(2), { close: true }); // [caught page, handoff]
  assert.deepEqual(handoffExit(3), { back: 2 }); // [other page, caught page, handoff]
  assert.deepEqual(handoffExit(10), { back: 2 });
});

test("handoffUrl percent-encodes the original URL", () => {
  const u = handoffUrl("https://music.youtube.com/watch?v=a&list=b");
  assert.equal(u, "mixtape://open?url=https%3A%2F%2Fmusic.youtube.com%2Fwatch%3Fv%3Da%26list%3Db");
  assert.ok(!u.slice("mixtape://open?url=".length).includes("&"));
  assert.ok(!u.slice("mixtape://open?url=".length).includes("?"));
});
