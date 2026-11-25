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

// True when the page the tab is on now says the link came from outside YouTube
// (a new tab, a typed address, another site or app). Undefined means the browser hides the
// tab's URL from us, which happens when the tab is not on a YouTube host.
export function fromOutside(currentTabUrl) {
  if (!currentTabUrl) return true;
  if (currentTabUrl === "about:blank") return true;
  if (/^(chrome|edge):\/\/newtab\/?/.test(currentTabUrl)) return true;
  const u = parse(currentTabUrl);
  if (!u) return true;
  return !isYoutubeHost(u.hostname.toLowerCase());
}

export function handoffUrl(url) {
  return "mixtape://open?url=" + encodeURIComponent(url);
}
