# Mixtape Link Catcher

A small Manifest V3 extension for Chrome and Edge. A YouTube Music link opened from outside the site
(a new tab, a typed address, another site or app) is handed to Mixtape through
`mixtape://open?url=...` and the tab closes. Clicks inside YouTube, and any other site, are left alone.

**Extension id: `njfkgiakgfoadoknhlokkcjapjpanfga`** (pinned by the `key` in `manifest.json`; the
private key was discarded).

## Install

1. Open `chrome://extensions` (Edge: `edge://extensions`).
2. Turn on developer mode.
3. «Загрузить распакованное» and pick this folder (Mixtape Settings, «Расширение для браузера», opens it).
4. The first time, the browser asks «Open Mixtape?». Tick «Always allow», then press
   «Браузер больше не спрашивает» on the extension's page.

## Privacy

The extension hears only YouTube navigations (URL filter), reads the tab's current address only to see
whether the link came from outside, and stores nothing but its two switches and one flag in
`chrome.storage.local`. No browsing history is kept or sent anywhere.

## Settings (toolbar popup)

- «Перехватывать и обычный YouTube» (off): also catch youtube.com watch, shorts and youtu.be links.
- «Пауза» (off): catching stops; the badge shows II.

Tests: `node --test "tools/link-catcher-tests/*.test.js"` (kept outside this folder because it ships).
