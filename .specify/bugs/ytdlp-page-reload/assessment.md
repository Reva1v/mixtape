# Bug Assessment: every track fails with «yt-dlp: The page needs to be reloaded»

- **Slug**: ytdlp-page-reload
- **Created**: 2026-11-23
- **Source**: pasted text + https://github.com/yt-dlp/yt-dlp/issues/17389 (host github.com, allowlisted; read via `gh`)
- **Verdict**: valid
- **Severity**: critical

## Report

Toast «Ошибка воспроизведения: yt-dlp: The page needs to be reloaded.» on every track, in Mixtape and in Droplet Island
(both run `D:\Software\Mixtape\yt-dlp.exe`, version 2026.08.19). `island.log.2026-11-23`: from 15:46Z every
`mixtape_player::player: playback error id=… yt-dlp: The page needs to be reloaded.`, and
`a YouTube request was refused, yet the session is signed in` (InnerTube with cookies works).

yt-dlp #17389 «The page needs to be reloaded (tv_downgraded player response playability status: UNPLAYABLE)», closed
2026-10-10: with cookies yt-dlp picks the `tv_downgraded` client, which YouTube now refuses. 2026.10.21 adds
`tv_samsung` and makes it a default client.

## Symptom

With an account connected nothing plays: yt-dlp can't resolve any stream. Expected: tracks play.

## Reproduction

1. Connect a YouTube account (cookies in `account.bin`).
2. Play any track: yt-dlp 2026.08.19 runs with `--cookies` and fails.
3. The same command without `--cookies` resolves format 251 (checked by hand on TbihvQ3GjTw).

## Suspected Code Paths

- `scripts/fetch-sidecars.ps1` — pins yt-dlp 2026.08.19, the installer bundles it.
- `crates/yt/src/resolver.rs:run_batch()` — one yt-dlp run with cookies; a tool error is final, no anonymous retry.
- `crates/yt/src/resolver.rs:classify()` — the message falls into `YtError::Tool`, shown raw in the toast.

## Root Cause Hypothesis

YouTube refuses yt-dlp's `tv_downgraded` client, which yt-dlp 2026.08.19 uses when cookies are passed. Confidence: high
(matching upstream issue, anonymous run works, signed-in InnerTube works).

## Proposed Remediation

**Preferred**:
1. Pin yt-dlp 2026.10.26 (SHA-256 `f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d` from its
   SHA2-256SUMS) in `scripts/fetch-sidecars.ps1`, notices and plan; release a new Mixtape version so the installed
   sidecar (also used by the island) is replaced.
2. In `run_batch`, when the run used cookies and some ids ended in `YtError::Tool`, resolve those ids once more without
   cookies and keep any success. Auth/Unavailable/age/region errors are not retried. Log a warning once per batch.
   Public tracks keep playing the next time YouTube breaks a signed-in client.

**Alternatives**:
- Pass `--extractor-args youtube:player_client=…`: fixes today's client, breaks again on the next change, and fights
  yt-dlp's own defaults.

**Files likely to change**: `scripts/fetch-sidecars.ps1`, `THIRD-PARTY-NOTICES.md`, `specs/001-mixtape-mvp/plan.md`,
`crates/yt/src/resolver.rs` (+ its tests).

**Tests to add or update** (fake yt-dlp, as the existing resolver tests do):
- cookie run fails with a tool error → anonymous run succeeds → result Ok, second call has no `--cookies`;
- cookie run fails with an auth error → no retry;
- no cookies set → no retry on a tool error (one call);
- mixed batch: only the tool-error ids are retried, the ok ones keep their first result.

## Risks & Considerations

- Anonymous streams can't play private uploads or premium quality; that only replaces an outright failure.
- A retry costs one more yt-dlp start (~1–3 s) only on failure.

## Open Questions

- None.
