// Which links Mixtape takes over. No dependencies; the same file is imported by the tests.

const YTM_HOST = "music.youtube.com";
const YT_HOSTS = ["www.youtube.com", "youtube.com", "m.youtube.com"];

function parse(url) {
  try {
    return new URL(url);
  } catch {
    return null;
  }
}

function isYoutubeHost(host) {
  return host === "youtu.be" || host === "youtube.com" || host.endsWith(".youtube.com");
}

// True when Mixtape should open this link instead of the browser.
export function catchable(url, { catchYoutube = false } = {}) {
  const u = parse(url);
  if (!u || (u.protocol !== "https:" && u.protocol !== "http:")) return false;
  const host = u.hostname.toLowerCase();
  const path = u.pathname;
  if (host === YTM_HOST) {
    if (path === "/watch") return u.searchParams.has("v");
    if (path === "/playlist") return u.searchParams.has("list");
    return /^\/browse\/(MPREb_|VL)[^/]+/.test(path) || /^\/channel\/UC[^/]+/.test(path);
  }
  if (!catchYoutube) return false;
  if (YT_HOSTS.includes(host)) {
    if (path === "/watch") return u.searchParams.has("v");
    return /^\/shorts\/[^/]+/.test(path);
  }
  if (host === "youtu.be") return /^\/[^/]+/.test(path);
  return false;
}

// A YouTube host (the tab shows a YouTube page), for the per-tab "last page was YouTube" note.
export function youtubeUrl(url) {
  const u = parse(url || "");
  return !!u && isYoutubeHost(u.hostname.toLowerCase());
}

// Transitions that say the link came from outside the site: another app (`auto_toplevel`),
// a typed address, the omnibox, a bookmark, the start page.
const OUTSIDE = new Set(["auto_toplevel", "typed", "generated", "keyword", "keyword_generated", "auto_bookmark", "start_page"]);

// Whether a committed main-frame YouTube navigation came from outside YouTube.
// Decided from the commit itself, so it doesn't depend on how fast the service worker woke up.
// `prevWasYoutube`: this tab's previous page was a YouTube page.
export function caughtCommit({ transitionType, transitionQualifiers = [], prevWasYoutube = false }) {
  if (transitionQualifiers.includes("forward_back")) return false;
  if (OUTSIDE.has(transitionType)) return true;
  if (transitionType === "link") return !prevWasYoutube;
  return false; // reload, form_submit, subframes and the rest
}

// Where the handoff page goes when done. The caught page has already committed, so it sits one entry
// before the handoff page: step past it, or close the tab when nothing came before it.
export function handoffExit(historyLength) {
  return historyLength > 2 ? { back: 2 } : { close: true };
}

export function handoffUrl(url) {
  return "mixtape://open?url=" + encodeURIComponent(url);
}
