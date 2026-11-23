# 003 addendum — delayed, verified auto-update of yt-dlp

Owner, 2026-11-24 chose «auto with a delay» over notice-only: Mixtape installs a new stable yt-dlp by itself, but only
once the release is at least 5 days old and only after it passes every check below. The notice (spec.md) stays: it
now says the update will install itself on <date>, or that it was installed.

## Rules

1. **Delay.** Candidate = GitHub `releases/latest` (stable, not draft/prerelease) newer than the yt-dlp in use, with
   `published_at` ≥ 5 days ago (`MIN_AGE`). Younger → notice only, «установится сама после {date}».
2. **Download** to `%LOCALAPPDATA%\Mixtape\tools\staging\` (never next to the running exe): `yt-dlp.exe`,
   `SHA2-256SUMS`, `SHA2-256SUMS.sig`, only from the release's own asset URLs on `github.com` /
   `objects.githubusercontent.com` / `release-assets.githubusercontent.com`; size caps (exe ≤ 64 MiB, sums ≤ 64 KiB,
   sig ≤ 8 KiB); https only.
3. **Signature.** Verify `SHA2-256SUMS.sig` (detached binary OpenPGP) over `SHA2-256SUMS` with yt-dlp's public key,
   **embedded in Mixtape** (`crates/yt/keys/yt-dlp-public.key`, copied from github.com/yt-dlp/yt-dlp `public.key`; note
   the source commit and fingerprint in a comment). Pure-Rust OpenPGP crate (e.g. `pgp`/rPGP); no gpg.exe. Fail → drop.
4. **Hash.** SHA-256 of the downloaded exe must equal the `yt-dlp.exe` line of the verified sums. Fail → drop.
5. **Smoke test** of the staged exe: `--version` prints exactly the release tag; then one anonymous resolve
   (`-J -f FORMATS`, no download) of a fixed public video succeeds. Fail → drop, remember the version as bad.
6. **Install.** Move the current tools copy (if any) to `tools\previous\`, staged exe to `tools\yt-dlp.exe`, write
   `tools\yt-dlp.json` `{ version, sha256, installed_at, previous }`. The resolver switches at the next call
   (no restart). Never touch `D:\Software\Mixtape\` (the installer's files).
7. **Which exe runs.** `host.sidecar("yt-dlp")` → `tools\yt-dlp.exe` if its recorded version is newer than the bundled
   one and its SHA-256 still matches `yt-dlp.json`; else the bundled one. A later Mixtape release with a newer bundled
   yt-dlp wins automatically.
8. **Rollback.** After an install, if 3 batches in a row fail on the new exe with retryable errors on both the cookie
   and the anonymous run while the previous exe resolves the same ids → restore `previous`, mark the version bad,
   toast «Новая yt-dlp {v} не работала, вернул {old}». Bad versions are never installed again; a newer one may be.
9. **Setting** «Обновлять yt-dlp автоматически (через 5 дней после выхода)», default on, in Settings → Updates; off →
   notice only. Settings shows the yt-dlp in use, its source (bundled / updated) and a «Вернуть встроенную» button that
   deletes `tools\yt-dlp.exe`.
10. **Logs** at info for every step and every refusal reason; toasts only for installed / rolled back.

## Tests (no network)

Pure functions get unit tests: age gate (exactly 5 days, 4d23h, future date, bad date), asset URL allow-list
(http, other host, lookalike host), size caps, sums parsing (missing line, two lines, bad hex), signature verify with a
test key pair generated in the test (good, tampered sums, wrong key, garbage sig), choose-which-exe (newer tools /
older tools / hash mismatch / no json), rollback counter (resets on success, needs both runs failing, needs the old exe
to succeed), bad-version list. Install/rollback file moves tested in a temp dir. The real public key parses and its
fingerprint matches the constant.
