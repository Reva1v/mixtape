import { handoffExit, handoffUrl } from "./rules.js";

const original = new URLSearchParams(location.search).get("url") || "";
const link = document.getElementById("link");
const hint = document.getElementById("hint");
const done = document.getElementById("done");
const browser = document.getElementById("browser");
link.textContent = original;

function closeSoon() {
  setTimeout(async () => {
    const exit = handoffExit(history.length);
    if (exit.back) {
      history.go(-exit.back);
    } else {
      const tab = await chrome.tabs.getCurrent();
      if (tab) chrome.tabs.remove(tab.id);
    }
  }, 1500);
}

const { promptAllowed = false } = await chrome.storage.local.get("promptAllowed");
if (promptAllowed) {
  closeSoon();
} else {
  hint.hidden = false;
  done.hidden = false;
}

done.addEventListener("click", async () => {
  await chrome.storage.local.set({ promptAllowed: true });
  closeSoon();
});

browser.addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "letThrough", url: original });
  location.href = original;
});

if (original) {
  location.href = handoffUrl(original);
}
