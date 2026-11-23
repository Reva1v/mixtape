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
