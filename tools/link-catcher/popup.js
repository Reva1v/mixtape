const defaults = { catchYoutube: false, paused: false };
const stored = await chrome.storage.local.get(defaults);
for (const key of Object.keys(defaults)) {
  const box = document.getElementById(key);
  box.checked = stored[key];
  box.addEventListener("change", () => chrome.storage.local.set({ [key]: box.checked }));
}
