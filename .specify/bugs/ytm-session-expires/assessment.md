# Bug Assessment: the YouTube Music session dies within hours and the toast repeats

- **Slug**: ytm-session-expires
- **Created**: 2026-11-02
- **Source**: pasted text and a screenshot from the user
- **Verdict**: valid
- **Severity**: high (the account drops: no likes sync, no mixes, no downloads, new login needed each time)

## Report

«у меня вылетает с акка гугла, я не знаю почему, возможно из-за дроплет айленд, проверь всё и нужно зафиксить».
Screenshot: Settings shows «Не подключён», two toasts «Сессия YouTube Music истекла — загрузки остановлены».

## Symptom

A session captured at sign-in stops being signed in within hours; every later request is answered as a guest.
Expected: the session lasts until the user signs out.

## Measured (2026-11-02, Mixtape 0.2.5, cookie values never printed)

- `account.bin` written 18:58:28Z, the sign-in. By 21:06Z (screenshot) the app reported expiry: under 2 h 8 min of life.
- With that jar at 21:08Z: `browse VLLM`, `browse FEmusic_home` and `account/account_menu` all return 200 with
  `logged_in=0`. Dropping `*PSIDRTS` changes nothing. So the session is really dead, not misparsed.
- `RotateCookies` on the dead jar: 200, sets `SIDCC`/`*PSIDCC` only, no `*PSIDTS`; still `logged_in=0`. A dead
  session cannot be revived, so the user must sign in once more whatever the fix.
- `__Secure-1PSIDRTS`/`3PSIDRTS` were issued with about a 10-minute life (expired 2 h before the check).
- The island's log has no sign of it writing the account; it read `account.bin` at 18:20:47Z and never again.

## Reproduction

1. Connect the account in Mixtape.
2. Leave the app running for a couple of hours.
3. `logged_in` turns 0; the toast appears and repeats. [NEEDS CLARIFICATION: exact lifetime, measured after the fix]

## Suspected Code Paths

- `crates/yt/src/innertube.rs:109` `call()`: `Set-Cookie` of every answer is dropped.
- `crates/yt/src/resolver.rs:329` `resolve_batch`: yt-dlp saves what it got into a temporary copy, deleted unread.
- `crates/session/src/account.rs:135`: `account.bin` is written only at sign-in.
- `crates/session/src/login_window.rs:4`: the header claims Google does not rotate an incognito session. Nothing here
  ever calls `RotateCookies`, which an open YouTube tab calls every 10 minutes.
- `crates/session/src/background.rs:52` and the other `account:expired` emitters: each failure emits again; the likes
  check keeps going every 20 s with a dead jar, hence the repeating toast.
- `crates/yt/src/resolver.rs:899`: any yt-dlp message containing «sign in» is `Auth`, including «Sign in to confirm
  you're not a bot», so a live session can be reported expired.
- `crates/session/src/account.rs:100` `status()`: after expiry a reload reports the account connected again.
- `droplet-island/src-tauri/src/music/mod.rs:607`: the island calls `account::restore` once per run, so a new sign-in
  in Mixtape does not reach it.

## Root Cause Hypothesis

Confidence: medium. Not yet proven against a live session; that takes a fresh sign-in and hours.

1. **(Preferred, fix targets this)** The jar is never refreshed. Prediction: with rotation stored, a session outlives
   the 2 h 8 min seen today. Community findings agree (music-assistant/server#7075, a98c14/youtube-download-helper#68,
   dankhael/fuuka-cata-link#30); none are official Google documentation.
2. Polling every 20 s since 0.2.5 flags the session. Prediction: with rotation in place it still dies; slowing the
   check helps. Next suspect if (1) fails.
3. Cookie pickup races the open sign-in page, which rotates after we read. Prediction: sessions die within minutes,
   not hours. Closed cheaply by closing the window before use.
4. Sign-in and later requests leave through different IPs (VPN). Prediction: dies regardless of rotation.
   [NEEDS CLARIFICATION: is a VPN in use?]

**The island is not the cause of the expiry**: it does not poll while Mixtape owns the session and does not write the
account, and its copy is the same jar. It does keep a stale jar after a re-login, a separate defect fixed here too.

## Proposed Remediation

**Preferred**: keep the jar alive the way a browser tab does, and make expiry truthful.

1. `Cookies::merge_set_cookies`: applies `Set-Cookie`, never deleting a cookie and ignoring empty or expired values
   (Google clears cookies when it invalidates a session).
2. `InnerTube`: merges answers' `Set-Cookie` unless the answer is a guest one; a generation counter drops answers to
   requests sent with a replaced jar; `rotate()` calls `accounts.youtube.com/RotateCookies`.
3. `resolver`: reads the rotated session cookies back from yt-dlp's copy and hands them to the session.
4. `account`: every 10 minutes while this process owns the session, re-reads `account.bin` if another process changed
   it, rotates, and saves atomically when something changed.
5. `account::auth_failed`: every emitter goes through it; a failure is confirmed with one signed-in request, then
   announced once per jar. While expired, the checks stop and `status()` says not connected.
6. Sign-in window closes right after the cookies are read.
7. Island: runs the keep-alive tick under its owner rule and picks up a new jar from disk.

**Alternatives**:
- Keep a hidden WebView2 logged in and export from it: closest to a real browser, but heavy and always running.
- Slow polling only: does not address the missing refresh.

**Files likely to change**:
- `crates/yt/src/{auth,innertube,resolver}.rs`
- `crates/session/src/{account,background,login_window,downloads,state}.rs`, `crates/session/src/commands/{mix,sync,catalog,home}.rs`
- `src-tauri/src/lib.rs`
- `droplet-island/src-tauri/src/music/{mod,likes}.rs`

**Tests to add or update**:
- `Set-Cookie` merge: replaces values, adds new, ignores deletions, other domains, and empty values; round trip through Netscape.
- A guest answer does not merge; an answer to a replaced jar does not merge (local HTTP server if the client allows a base URL).
- yt-dlp read-back takes only rotating session cookies.
- `auth_failed` announces once per jar and again after a new sign-in.

## Risks & Considerations

- Rotating and yt-dlp can race; last writer wins for short-lived values, and Google accepts the previous value.
- Two processes rotating separately would kill each other's jar: only the owner rotates, the file is the shared truth.
- `RotateCookies` is undocumented and may change; a failed rotation only logs, and never marks the session expired.
- `mixtape-android` uses these crates as git deps: API changes must be additive.
- Island releases separately; until it updates, its idle check uses a stale jar and only the expiry notice is affected.

## Open Questions

- [NEEDS CLARIFICATION: is a VPN or proxy in use? Relevant to hypothesis 4 only.]
