# Fix: ytdlp-page-reload

## What changed

1. yt-dlp pin raised from 2026.08.19 to 2026.10.26 (SHA-256
   `f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d`). Newer yt-dlp has the `tv_samsung` default client,
   so cookies no longer select the refused `tv_downgraded` client.
2. `StreamResolver::run_batch` (`crates/yt/src/resolver.rs`): when the run used cookies and some ids ended in
   `YtError::Tool`, those ids are resolved once more with `run_ytdlp(&retry_ids, None)`. An id's result is replaced only
   when the anonymous result is Ok. Auth, rate-limit, unavailable and other variants are never retried; no cookies means
   no retry. One `tracing::warn!` per batch with the retried count. The cookie-return hook is not called for the
   anonymous run.

## Files

- `scripts/fetch-sidecars.ps1`, `THIRD-PARTY-NOTICES.md`, `specs/001-mixtape-mvp/plan.md` (pin)
- `crates/yt/src/resolver.rs` (retry + tests)

## Tests (fake yt-dlp, Windows only, no network)

- `tool_error_with_cookies_is_retried_anonymously`: second call has no `--cookies`, result Ok.
- `rate_limit_with_cookies_is_not_retried`: "not a bot" error, exactly one call.
- `tool_error_without_cookies_is_not_retried`: one call.
- `mixed_batch_retries_only_the_failed_id`: ok id keeps first result, only the failed id is in the second call.
- `failed_anonymous_retry_keeps_the_original_error`: original tool error kept.

The installed sidecar is replaced only by releasing a new Mixtape version.

## Follow-ups (2026-11-23/24)

3. **0.4.4, `f4bbe10`.** YouTube's player `b17b74e6` (2026-11-23) broke yt-dlp 2026.10.26's n challenge for signed-in
   clients: only images, «Requested format is not available» → `Unavailable`, which item 2 did not retry
   (yt-dlp #18123, fixed in yt-dlp master, no stable release yet). The retry now covers every non-final error:
   `retry_anonymously()` = `Tool | Unavailable | Parse`; `Auth`, `RateLimited`, `Network` stay final.
4. **0.4.5, `905f08e`.** Each track waited for a cookie run bound to fail (~6 s) before the anonymous one. A batch
   rescued anonymously opens a 30-minute window (`anon_until`, `ANON_FIRST_SECS`) in which runs start without cookies
   and fall back to cookies only for retryable failures (private uploads); after it cookies go first again.
5. **`27466f8`.** The startup session check (`status` → `first_look`) logged as «a YouTube request was refused, yet the
   session is signed in»; it now goes through `verify(app, None)` and logs at debug.

Tests added: `missing_format_with_cookies_is_retried_anonymously`, `refused_sign_in_with_cookies_is_not_retried`,
`only_final_errors_skip_the_anonymous_retry`, `after_a_rescue_runs_start_without_cookies`,
`anonymous_first_failure_falls_back_to_cookies`, `failed_rescue_keeps_cookies_first`,
`expired_window_goes_back_to_cookies`. Mutation check: forcing `anon_first = false` fails two of them.
`dropped_call_kills_the_process_tree` / `timeout_kills_the_process_tree` are timing-sensitive and failed once under a
parallel island build; they pass alone and on a rerun.

**Next:** when yt-dlp ships a stable release after 2026.11.23 (fix for #18123), bump the pin in
`scripts/fetch-sidecars.ps1` (version + SHA-256 from SHA2-256SUMS), replace `src-tauri/binaries/yt-dlp-*.exe`
and release; the anonymous fallback then stops kicking in by itself.

Live check: PC Mixtape 0.4.4+ plays with the account (owner, 2026-11-24); island fixed from 1.22.2.
