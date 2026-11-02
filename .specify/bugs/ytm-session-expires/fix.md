# Bug Fix: the YouTube Music session dies within hours and the toast repeats

- **Slug**: ytm-session-expires
- **Fixed**: 2026-11-02
- **Assessment**: ./assessment.md
- **Status**: applied (lifetime against a live session not yet measured)

## Summary

The stored session is now refreshed like an open YouTube tab does it, what YouTube changes is kept, and an expiry is
confirmed and announced once. Toasts no longer pile up.

## Changes

| File | Change | Notes |
|------|--------|-------|
| `crates/yt/src/auth.rs` | modified | `merge_set_cookies`, `merge_rotating`; deletions are ignored |
| `crates/yt/src/innertube.rs` | modified | answers' `Set-Cookie` kept unless the answer is a guest one or the jar was replaced; `rotate()` |
| `crates/yt/src/resolver.rs` | modified | hands yt-dlp's cookie file back before deleting it |
| `crates/session/src/account.rs` | modified | `keep_alive`, `reload_from_disk`, `auth_failed`, `usable`, atomic save |
| `crates/session/src/background.rs` | modified | keep-alive loop every 10 minutes |
| `crates/session/src/login_window.rs` | modified | window closes before the jar is used |
| `crates/session/src/state.rs` | modified | `account` health, yt-dlp hook |
| `crates/session/src/downloads.rs`, `commands/{mix,sync,catalog}.rs` | modified | emitters go through `auth_failed` |
| `ui/src/lib/stores/overlay.svelte.ts` | modified | one toast per message, at most three |
| `ui/src/lib/ui/Toaster.svelte` | modified | opaque, one line when it fits, left-aligned text |
| `droplet-island/src-tauri/src/music/likes.rs` | modified | other repo: keep-alive as owner, re-reads the jar |

## Tests Added or Updated

- `crates/yt/src/auth.rs` — merge replaces, adds, never clears, date parsing, yt-dlp read-back limits.
- `crates/yt/src/innertube.rs` — guest answers and answers to a replaced jar are dropped.
- `crates/session/src/account.rs` — expiry announced once per jar.
- `ui/src/lib/stores/overlay.test.ts` — one toast per message, timer extended, cap of three.

## Local Verification

- `cargo test -p mixtape-yt -p mixtape-session --lib` → 149 passed.
- `pnpm check` → 0 errors; `vitest overlay.test.ts` → 4 passed.
- `cargo test -p droplet-island --lib music::` → 26 passed; `--test music_link` → 5 passed.
- `RotateCookies` answered 200 for a dead jar with `SIDCC` only, checked by hand; never run against a live session.

## Deviations from Assessment

- The island edit is outside this repo, against this command's workspace guardrail; the user asked to check and fix
  the island together with the app.
- Toast UI added at the user's request during the fix.
- `auth_failed` adds a toast when downloads pause while the session is still signed in, so the pause is not silent.
- The planned HTTP-level test was replaced by tests of `absorb_answer`: the client has no base URL to point elsewhere.

## Follow-ups

- Measure a fresh session's lifetime; if it still dies, suspect the 20-second likes polling next (assessment, hypothesis 2).
