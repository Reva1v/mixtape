# Plan — 003 yt-dlp update notice

## Pieces

- `crates/yt/src/ytdlp_release.rs` (pure, light crate, unit-tested):
  - `parse_version(&str) -> Option<Vec<u32>>` — trims, splits on `.`, all parts numeric, 3–4 parts.
  - `is_newer(latest: &str, installed: &str) -> bool` — false when either is unparsable.
  - `latest_tag(json: &str) -> Option<String>` — `tag_name` from the GitHub release JSON; rejects drafts/prereleases
    (`draft`/`prerelease` true).
  - `async fn fetch_latest(http: &reqwest::Client) -> Result<String>` — the GET with `User-Agent: Mixtape`,
    `Accept: application/vnd.github+json`, 15 s timeout, body ≤ 512 KiB (reuse `innertube::read_limited` if public,
    else a local limit).
  - `async fn installed_version(ytdlp: &Path) -> Result<String>` — runs `ytdlp_command(...)` with `--version`
    (CREATE_NO_WINDOW, kill on drop, 20 s timeout), first line trimmed.
- `src-tauri/src/updates.rs`: a second loop (`spawn_ytdlp`) next to `spawn`: 60 s after start, then daily. Emits
  `ytdlp:available` with `YtdlpInfo { installed, latest }` (ts-rs export) when newer; command `ytdlp_check` returns
  `YtdlpStatus { installed: Option<String>, latest: Option<String>, newer: bool }` for Settings. Register in
  `build.rs` COMMANDS + capability like other commands.
- Shown-once: UI keeps `ytdlpAnnounced` in localStorage; the toast is skipped when equal to `latest`.
- UI: `ui/src/lib/stores/updates.svelte.ts` listens to `ytdlp:available` → toast (info, 20 s, no action button);
  `Settings.svelte` Updates section gets the yt-dlp line + check button. i18n keys in en/uk/ru.

## Tests

- Rust unit: version parse (3/4 parts, spaces, garbage, empty, leading `v`→ reject), `is_newer` (equal, older, newer by
  day, by 4th part, unparsable either side), `latest_tag` (normal, prerelease, draft, missing field, bad JSON).
- `installed_version` with a fake `yt-dlp.cmd` printing `2026.10.26` (Windows, like resolver tests); failing script →
  error.
- No network in tests. UI: vitest for the shown-once logic if it is a pure helper.
