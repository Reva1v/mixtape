# Bug Assessment: the session still ends after 2 hours; the island shows a dead account as connected

- **Slug**: ytm-session-two-hours
- **Created**: 2026-11-03
- **Source**: pasted text from the user, follow-up to ytm-session-expires
- **Verdict**: valid
- **Severity**: high

## Report

«у меня в основном приложении вылетает аккаунт 100% из-за дроплет айлонда, я захожу в настройка микстейпа в дроплет айленде и там есть профиль гугла, а когда захожу в отдельную программу, то вижу снова ошибку и что нет аккаунта».

## Symptom

The island's Mixtape settings show the Google profile while the standalone app says the account is gone.
Expected: both tell the truth, and the session lasts.

## Measured (2026-11-03, Mixtape 0.3.0, island 1.12.3; cookie values never printed)

- `account.bin` now: `browse VLLM` answers 200 with `logged_in=0`. The session is dead for both apps.
- By its cookie expiries the jar is the sign-in of 22:07Z, not a new one.
- That sign-in: alive at 22:10Z; island log `YouTube session is signed out` at 00:07:30Z. The island checks every 2 min, so
  it died between 00:05:30Z and 00:07:30Z: 2 h 00 after sign-in, with rotation running.
- Previous one: signed in 18:58Z, seen dead at 21:06Z. Fits 2 h.
- `__Secure-*PSIDRTS` were issued for 10 minutes and were never renewed by `RotateCookies` (expired 13.7 h ago).
- Island log: `account changed by another process` at 11:55:17Z, `signed out` at 11:59:17Z. A write of the same dead jar
  woke the other process, which forgot it was dead, rotated, wrote and announced again.

## Suspected Code Paths

- `crates/session/src/login_window.rs:47` — sign-in in Chromium, which can bind the session to the device.
- `crates/session/src/account.rs` `status()` — «connected» means only that a jar exists; a non-owner process never asks.
- `crates/session/src/account.rs` `reload_from_disk` — any changed bytes revive a dead jar.
- `crates/session/src/account.rs` `persist_changes` — writes a dead jar.

## Root Cause Hypothesis

1. Medium confidence: Device Bound Session Credentials. The sign-in happens in Chromium, the session is bound to its key, its
   short-lived `*PSIDRTS` can be renewed only by that browser, and Google ends it after about 2 hours. DBSC's mechanism is
   documented; this cookie's link to it and the 2 hours are inferred. Prediction: signed in with DBSC off, the jar has no
   `*PSIDRTS` and the session outlives 2 hours.
2. Polling from a non-browser client flags the session. Prediction: it still dies at varying times.
3. The island kills it. Evidence against: the island writes the same session's jar, and the death time is exactly 2 h whoever
   owned the session (Mixtape yesterday, the island last night). Not ruled out by experiment.

High confidence for the display and the ping-pong: read directly from the code and the log.

## Proposed Remediation

**Preferred**: sign in with DBSC off in a window of its own data folder; ask YouTube once per jar before calling it connected;
keep a dead session dead across refresh writes; do not write dead jars.

**Alternatives**:
- A hidden WebView2 kept signed in, exporting its jar: survives DBSC, but keeps a browser running, against the performance budget.

**Files likely to change**: `crates/session/src/{account,login_window}.rs`, `crates/yt/src/auth.rs`.

**Tests to add or update**: `same_session` for a refreshed jar and a new sign-in; a jar is looked at once until a new one.

## Risks & Considerations

- The feature names are Chromium's and not a WebView2 contract; a runtime update may ignore them. The jar shows it: `*PSIDRTS`
  present after sign-in means the switch did nothing.
- Only a fresh sign-in and over 2 hours prove hypothesis 1.

## Open Questions

- None blocking.
