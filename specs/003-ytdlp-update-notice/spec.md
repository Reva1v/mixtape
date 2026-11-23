# 003 — Notice when a new yt-dlp is out

Owner, 2026-11-24: no auto-update of yt-dlp (a compromised release must never install itself on the PC). Mixtape only
tells the owner that a new stable yt-dlp exists; the owner brings it to Claude, who checks the release (changelog,
SHA-256 from SHA2-256SUMS), bumps `scripts/fetch-sidecars.ps1`, tests and releases Mixtape.

## User stories

1. **Notice.** When yt-dlp publishes a stable release newer than the one installed next to Mixtape, Mixtape shows a
   toast once per new version: «Вышла новая yt-dlp {latest} (у вас {installed}). Сама она не ставится — попросите
   Claude проверить и обновить». (uk/en too.)
2. **Settings → Updates** shows a line with the installed yt-dlp version and, when newer exists, the latest one plus the
   same hint. A «Проверить» there checks by hand.
3. Nothing is downloaded or installed, ever. Only the public GitHub API is read.

## Acceptance

- Check runs a minute after start, then once a day, regardless of the app auto-update setting.
- Installed version = output of the bundled `yt-dlp --version` (what really runs), read once per check.
- Latest = `tag_name` of `GET https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest` (stable only; nightly
  builds live in another repo). Request has a User-Agent `Mixtape`, a timeout, a body limit.
- Versions compare numerically by dot parts (`2026.10.26` < `2026.11.25` < `2026.11.25.1`); unparsable → no notice.
- Offline / API error / rate limit → logged at info, no toast, retried next day.
- The toast for a version shows once: the version last announced is kept (prefs or settings table).
- The Android app is out of scope (no yt-dlp there). Droplet Island is out of scope.
