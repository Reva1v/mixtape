# Implementation Plan: Hand Mixtape links to Droplet Island

**Branch**: `005-island-link-handoff` | **Date**: 2026-11-26 | **Spec**: [spec.md](spec.md)

## Summary

Before Tauri starts, `run()` reads the command line. If it holds Mixtape links and no `--from-island`, it looks
for the current session's `droplet-island.exe`; when found, it starts that program once per link with
`--mixtape-link <link>` and exits with code 0 before any window, tray, session or state is touched. Links it
could not hand over (no island, spawn failed, invalid link) go on to the normal 0.6.0 path.
A second launch goes through the same `run()` first, so a running Mixtape is never involved when the island
takes the link (FR-003 for free). The single-instance callback and the cold-start path learn the
`--mixtape-link <link>` form.

## Technical Context

- Rust 2024, Tauri 2 shell `src-tauri` (crate `mixtape`), `mixtape-win` for Win32.
- Process lookup: Toolhelp32 snapshot (`CreateToolhelp32Snapshot` / `Process32FirstW` / `Process32NextW`),
  `ProcessIdToSessionId` for the current session, `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` +
  `QueryFullProcessImageNameW` for the path. Only a new feature flag of the existing `windows` 0.62 crate
  (`Win32_System_Diagnostics_ToolHelp`), no new dependency.
- Spawn: `std::process::Command::new(path).args(["--mixtape-link", link]).spawn()` — Rust quotes each argument
  for `CommandLineToArgvW`, so `&`, `?`, `%` and spaces in the link survive.
- Not used: the hand-off pipe (`mixtape-<SID>-island`). Connecting to it as the app side would replace the real
  Mixtape's link on the island (`keep_new`) and could move playback.

## Constitution Check

Windows only ✓, YouTube untouched ✓, no new dependencies ✓ (a feature flag of `windows`), headless tests ✓
(the process test finds the test binary itself; no island is started by tests).

## Design

`src-tauri/src/links.rs` (pure, unit-tested):
- `Start { links: Vec<String>, from_island: bool }`, `parse(args) -> Start`: bare `mixtape:` arguments and the
  argument after `--mixtape-link` (only when it starts with `mixtape:`), in order; `--from-island` anywhere.
- `handable(link) -> bool`: scheme `mixtape:` (any case) and `len() <= 4096`.
- `route(start, island: Option<&Path>, spawn: impl FnMut(&Path, &str) -> bool) -> Vec<String>`: the links that
  stay with Mixtape. Nothing is handed when `from_island` or no island; each handable link is handed with
  `spawn`; a failed spawn keeps that link and the ones after it (order is kept on one side).

`crates/win/src/process.rs`:
- `find_in_session(exe_name) -> Option<PathBuf>`: first process with that image name (case-insensitive) in the
  current session whose full path can be read. Never errors: any failure is `None`.

`src-tauri/src/lib.rs` `run()`: first thing after logging: `let start = links::parse(args)`; if it has links,
`let rest = links::route(&start, island_path, spawn_island)`; if `rest` is empty → `std::process::exit(0)`.
Otherwise remember `rest` (a `OnceLock`) for `setup` instead of re-reading `std::env::args()`. Single-instance
callback: `links::parse(args).links` (it never routes: its launcher already did).

## Project Structure

```
crates/win/src/process.rs      new: find_in_session + test
crates/win/src/lib.rs          pub mod process
crates/win/Cargo.toml          feature Win32_System_Diagnostics_ToolHelp
src-tauri/src/links.rs         parse / handable / route + tests
src-tauri/src/lib.rs           early routing in run(), setup uses the remembered links, callback uses parse
```

## Risks

- A process named `droplet-island.exe` that isn't the island: only a local program can do that and it already
  runs as the user; the link is its only payload. Accepted.
- The island is starting up and not ready yet: its own single-instance handles a second launch; its cold start
  queues the link (island spec).
