# Bug Fix: the session still ends after 2 hours; the island shows a dead account as connected

- **Slug**: ytm-session-two-hours
- **Fixed**: 2026-11-03
- **Assessment**: ./assessment.md
- **Status**: applied (the 2-hour limit not yet measured after the fix)

## Summary

Sign-in now runs with device bound sessions off. Each process asks YouTube once before showing a jar as connected, and a
dead session stays dead across refresh writes.

## Changes

| File | Change | Notes |
|------|--------|-------|
| `crates/session/src/login_window.rs` | modified | DBSC off, own data folder |
| `crates/session/src/account.rs` | modified | one look per jar, same session keeps its state, dead jars are not written |
| `crates/yt/src/auth.rs` | modified | `same_session` |

## Tests Added or Updated

- `auth::tests::a_refreshed_jar_is_the_same_session_and_a_new_sign_in_is_not`
- `account::tests::a_jar_is_looked_at_once_until_a_new_one`

## Local Verification

- `cargo test -p mixtape-yt -p mixtape-session --lib` → 157 passed.
- Not run: the sign-in window in a built app. A dev run would have claimed the session from the island and interrupted its
  music, so the first real run is the user's sign-in.

## Deviations from Assessment

None.

## Follow-ups

- After a sign-in, watch the session past 2 hours. `__Secure-1PSIDRTS` still in the jar means the switch did nothing.
- The island needs a release for the display fix; `account.rs` reaches it only through its build.
