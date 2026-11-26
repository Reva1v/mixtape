# Bug Assessment: browser extension doesn't catch a link opened from Telegram

- **Slug**: extension-misses-external-link
- **Created**: 2026-11-26
- **Source**: pasted text (owner's live check T038, Edge, screenshot of YouTube Music playing in the tab)
- **Verdict**: likely valid, needs reproduction (extension is installed, enabled and holds its permissions: checked in Edge's `Default/Secure Preferences`)
- **Severity**: high (the extension's only job; spec 004 US3)

## Report

Owner clicked a `music.youtube.com/watch?v=Fk8C1CcsJZY` link in Telegram Desktop with Mixtape Link Catcher
loaded unpacked in Edge (from `D:\Software\Mixtape\link-catcher`). Edge opened YouTube Music and played the
track; no `mixtape://` prompt, no handoff page. «ничего не спросило».

## Symptom

A YouTube Music link opened from another app loads in the browser instead of going to Mixtape.
Expected: the tab goes to `handoff.html`, which opens `mixtape://open?url=…` and closes.

## Reproduction

1. Edge with the extension loaded, its service worker asleep (the normal state after ~30 s idle).
2. Click a `https://music.youtube.com/watch?v=…` link in Telegram Desktop.
3. Edge opens a new tab with YouTube Music; nothing is caught.

## Suspected Code Paths

- `tools/link-catcher/background.js` `onBeforeNavigate` listener — awaits `chrome.storage.local.get`, then
  `chrome.tabs.get(tabId)`, and only then decides with `fromOutside(tab.url)`.
- `tools/link-catcher/rules.js:fromOutside()` — returns false once the tab's URL is a YouTube host.

## Root Cause Hypothesis

`onBeforeNavigate` doesn't block the navigation. With a cold service worker the event is delivered after
the worker starts, and two more async calls follow; by the time `tabs.get` runs, the navigation has
committed and `tab.url` is the YouTube Music page itself, so `fromOutside` says "inside YouTube" and the
link passes. The decision depends on timing, not on where the link came from. Confidence: medium-high
(the code path is clearly racy; not observed in the worker's console).

## Proposed Remediation

**Preferred**: decide on `webNavigation.onCommitted` (same YouTube-only URL filter), which carries
`transitionType` and `transitionQualifiers`, plus a per-tab note of "this tab's last commit was a YouTube page"
kept in `chrome.storage.session` (memory only, cleared when the tab is closed or shows a non-YouTube page,
which `tabs.onUpdated` reveals as `tab.url === undefined` without any extra permission). Catch when:
not `forward_back`, type is not `reload`/`form_submit`/subframe, and either the type says it came from
outside (`auto_toplevel` = another app, `typed`, `generated`, `keyword`, `auto_bookmark`, `start_page`) or it's a `link`
in a tab whose previous page wasn't YouTube. The pure rule goes into `rules.js` (`caughtCommit`) and is
unit-tested; the timing-dependent `tabs.get` goes away.
Because the YouTube page now commits before the redirect, `handoff.html` gets `back=2` and steps two
entries back (or closes the tab when nothing is before the link), so it never lands on the page it caught.

**Alternatives**:
- Keep `onBeforeNavigate`, swap `tabs.get` for the per-tab note: no transition type, so back/forward and
  session restore would be caught by mistake.
- Hear all navigations to know the previous page: breaks FR-011 (extension must not see other sites).

**Files likely to change**: `tools/link-catcher/background.js`, `rules.js`, `handoff.js`,
`tools/link-catcher-tests/rules.test.js`, `README.md` if behaviour text changes.

**Tests to add or update**: `caughtCommit` table — auto_toplevel / typed / link-from-outside caught;
link inside YouTube, reload, forward_back, form_submit not caught; `handoffBack` steps.

## Risks & Considerations

- The YouTube page commits for a moment before the redirect (no audio expected: playback starts after scripts).
- Session restore: restored tabs are reported as `reload` in Chromium; if Edge reports otherwise, restored
  YTM tabs would be caught. Check live.
- Installed copy at `D:\Software\Mixtape\link-catcher` comes from the installer; testing the fix before a
  release means loading `tools/link-catcher` from the repo instead.

## Open Questions

- None blocking.
