# Bug Fix: browser extension doesn't catch a link opened from Telegram

- **Slug**: extension-misses-external-link
- **Fixed**: 2026-11-26
- **Assessment**: ./assessment.md
- **Status**: applied

## Summary

The catch decision moved from `onBeforeNavigate` + `chrome.tabs.get` (racy: a sleeping worker sees the
already committed YouTube page) to `webNavigation.onCommitted`, deciding from the commit's transition type and
qualifiers plus an in-memory per-tab note "previous page was YouTube".

## Changes

| File | Change | Notes |
|------|--------|-------|
| `tools/link-catcher/rules.js` | modified | `fromOutside` replaced by `caughtCommit`, `youtubeUrl`, `handoffExit` |
| `tools/link-catcher/background.js` | modified | `onCommitted` listener (same YouTube-only filter); per-tab note in `storage.session`, cleared by `tabs.onUpdated` when the URL is hidden/non-YouTube and by `tabs.onRemoved` |
| `tools/link-catcher/handoff.js` | modified | exits via `handoffExit`: two entries back, or closes a tab that had nothing before the link |
| `tools/link-catcher/manifest.json` | modified | version 1.0.1 |
| `tools/link-catcher-tests/rules.test.js` | modified | tests for the new rules |

## Tests Added or Updated

- `caughtCommit`: outside transitions caught (also after YouTube); `link` caught only when the tab wasn't on YouTube;
  `forward_back`, `reload`, `form_submit`, subframes, empty never caught; `from_address_bar` alone doesn't block.
- `youtubeUrl`: YouTube hosts only; hidden, extension and look-alike hosts are not.
- `handoffExit`: 1–2 entries close the tab, 3+ step back two.

## Local Verification

- `node --test "tools/link-catcher-tests/*.test.js"` → 12 pass.
- `node --check` on the three scripts → ok.
- Copied to the installed folder `D:\Software\Mixtape\link-catcher` for the owner's live check in Edge.

## Deviations from Assessment

- None. `tabs` permission still not requested; FR-011 holds (only YouTube navigations are heard).

## Follow-ups

- Live check in Edge and Chrome (T038), including session restore of a YouTube Music tab.
- Ships to users with the next Mixtape release (the installer bundles `tools/link-catcher`).
