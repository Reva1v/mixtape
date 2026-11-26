import { caughtCommit, catchable, youtubeUrl } from "./rules.js";

// One navigation may pass untouched after "Открыть в браузере" (tab id -> url).
// Lives only in memory, holds at most one URL per tab, and is cleared when used.
const letThrough = new Map();

const FILTER = { url: [{ hostSuffix: "youtube.com" }, { hostEquals: "youtu.be" }] };

function settings() {
  return chrome.storage.local.get({ catchYoutube: false, paused: false });
}

async function updateBadge() {
  const { paused } = await settings();
  await chrome.action.setBadgeText({ text: paused ? "II" : "" });
  if (paused) await chrome.action.setBadgeBackgroundColor({ color: "#666666" });
}

// Per tab, in memory only (storage.session): was this tab's last page a YouTube page? Set on YouTube
// commits; cleared when the tab shows anything else, which tabs.onUpdated reveals only as "url hidden"
// (no tabs permission, so other sites' URLs never reach us), and when the tab closes.
const PREV = "prevYoutube";

async function prevYoutube() {
  const { [PREV]: map = {} } = await chrome.storage.session.get(PREV);
  return map;
}

async function setPrev(tabId, on) {
  const map = await prevYoutube();
  if (!!map[tabId] === on) return;
  if (on) map[tabId] = true;
  else delete map[tabId];
  await chrome.storage.session.set({ [PREV]: map });
}

// Only YouTube navigations ever reach this listener (the filter above). The decision uses the commit's
// transition, not the tab's current URL: by the time a sleeping worker wakes, the page has committed.
chrome.webNavigation.onCommitted.addListener(async (details) => {
  if (details.frameId !== 0) return;
  const map = await prevYoutube();
  await setPrev(details.tabId, true);
  const { catchYoutube, paused } = await settings();
  if (paused) return;
  if (letThrough.get(details.tabId) === details.url) {
    letThrough.delete(details.tabId);
    return;
  }
  if (!catchable(details.url, { catchYoutube })) return;
  const caught = caughtCommit({
    transitionType: details.transitionType,
    transitionQualifiers: details.transitionQualifiers,
    prevWasYoutube: !!map[details.tabId],
  });
  if (!caught) return;
  await chrome.tabs.update(details.tabId, {
    url: chrome.runtime.getURL("handoff.html?url=" + encodeURIComponent(details.url)),
  });
}, FILTER);

chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.status !== "loading" && !changeInfo.url) return;
  if (!youtubeUrl(tab.url)) setPrev(tabId, false);
});

chrome.tabs.onRemoved.addListener((tabId) => setPrev(tabId, false));

chrome.runtime.onMessage.addListener((msg, sender) => {
  if (msg && msg.type === "letThrough" && sender.tab) {
    letThrough.set(sender.tab.id, msg.url);
  }
});

chrome.storage.onChanged.addListener(updateBadge);
chrome.runtime.onStartup.addListener(updateBadge);
chrome.runtime.onInstalled.addListener(updateBadge);
