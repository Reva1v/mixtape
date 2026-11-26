# Bug Verification: browser extension doesn't catch a link opened from Telegram

- **Slug**: extension-misses-external-link
- **Verified**: 2026-11-26
- **Fix**: ./fix.md
- **Result**: resolved

## Checks

- Automated: `node --test "tools/link-catcher-tests/*.test.js"` → 12 pass (commit-rule, YouTube host and handoff-exit tables).
- Live (owner, Edge, extension 1.0.1 loaded from `D:\Software\Mixtape\link-catcher`): a music.youtube.com link
  clicked in Telegram Desktop opened the `mixtape://` prompt and played in Mixtape; after Mixtape 0.7.0 and
  island 1.23.4 the same click plays in the island. Owner: «работает», then «Всё работает».

## Not covered

- Chrome (same code; not clicked live yet).
- Session restore of a YouTube Music tab (assessment risk): not observed.
