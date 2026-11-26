# Tasks: Hand Mixtape links to Droplet Island

**Input**: [spec.md](spec.md), [plan.md](plan.md)

## Phase 1: Foundation

- [X] T001 [P] `crates/win/Cargo.toml`: add `Win32_System_Diagnostics_ToolHelp` to the `windows` features
- [X] T002 [P] `crates/win/src/process.rs` + `pub mod process` in `lib.rs`: `find_in_session(exe_name) -> Option<PathBuf>`
  (Toolhelp32 snapshot, case-insensitive name, same session as `GetCurrentProcessId`, path via
  `QueryFullProcessImageNameW`; handles closed). Tests: finds the test binary by its own file name; an unknown
  name and an empty name give `None`; name match ignores case

## Phase 2: US1 + US3 — routing (P1/P2)

- [X] T003 `src-tauri/src/links.rs`: `Start`, `parse(args)`: bare links, `--mixtape-link <link>` (value must start
  with `mixtape:`; a missing or non-link value is ignored), `--from-island`; keeps order. Tests: each form,
  mixed forms in order, flag at the end with no value, flag followed by a non-link, `--from-island` anywhere,
  `--autostart --hidden` alone gives no links
- [X] T004 `src-tauri/src/links.rs`: `handable(link)`: scheme case-insensitive, 4096 bytes allowed, 4097 not.
  Tests at the boundary
- [X] T005 `src-tauri/src/links.rs`: `route(start, island, spawn) -> Vec<String>`. Tests with a fake spawn:
  no island → all stay, nothing spawned; island → all handed, none stay, spawn gets path and link in order;
  `from_island` → all stay, nothing spawned; an unhandable link stays and doesn't stop the others; a failed
  spawn keeps that link and every later one, earlier handed ones are not repeated
- [X] T006 `src-tauri/src/lib.rs`: in `run()` before the builder: parse, route with
  `mixtape_win::process::find_in_session("droplet-island.exe")` and a spawn of `--mixtape-link <link>`; exit 0
  when nothing stays; keep the rest in a `OnceLock` used by `setup` (window shown when links stay, as today)
- [X] T007 `src-tauri/src/lib.rs`: single-instance callback uses `links::parse(args).links` (it never routes: the second launch already did); still raises the window as today

## Phase 3: Polish

- [X] T008 `cargo test -p mixtape-win -p mixtape`, `cargo clippy` on both, then the workspace gates once
- [X] T009 Docs: README ×3 «Opening links» gets one line about the island; `specs/001-mixtape-mvp/STATUS.md`
- [X] T010 (2026-11-26: owner confirmed «Всё работает» with Mixtape 0.7.0 + island 1.23.4: a YTM link from Telegram via the Edge extension plays in the island) Owner's live check (quickstart in spec Independent Tests) after the island side ships; release
