import { catchable, fromOutside } from "./rules.js";

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

// Only YouTube navigations ever reach this listener (the filter above). Nothing is stored.
chrome.webNavigation.onBeforeNavigate.addListener(async (details) => {
  if (details.frameId !== 0) return;
  const { catchYoutube, paused } = await settings();
  if (paused) return;
  if (letThrough.get(details.tabId) === details.url) {
    letThrough.delete(details.tabId);
    return;
  }
  if (!catchable(details.url, { catchYoutube })) return;
  let current;
  try {
    current = (await chrome.tabs.get(details.tabId)).url;
  } catch {
    return;
  }
  if (!fromOutside(current)) return;
  await chrome.tabs.update(details.tabId, {
    url: chrome.runtime.getURL("handoff.html?url=" + encodeURIComponent(details.url)),
  });
}, FILTER);

chrome.runtime.onMessage.addListener((msg, sender) => {
  if (msg && msg.type === "letThrough" && sender.tab) {
    letThrough.set(sender.tab.id, msg.url);
  }
});

chrome.storage.onChanged.addListener(updateBadge);
chrome.runtime.onStartup.addListener(updateBadge);
chrome.runtime.onInstalled.addListener(updateBadge);
