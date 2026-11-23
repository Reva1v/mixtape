# Tasks — 003 yt-dlp update notice

- [ ] T001 `crates/yt/src/ytdlp_release.rs`: `parse_version`, `is_newer`, `latest_tag` + unit tests (edge cases in plan).
- [ ] T002 same file: `fetch_latest` (GitHub API, UA, timeout, body limit) and `installed_version` (sidecar `--version`)
      + fake-script test for `installed_version`.
- [ ] T003 `src-tauri/src/updates.rs`: `spawn_ytdlp` loop, `ytdlp:available` event, `ytdlp_check` command, ts-rs types;
      register command (build.rs COMMANDS, capability), start the loop where `updates::spawn` starts.
- [ ] T004 UI: store listens + toast once per version (localStorage), Settings → Updates line + check button,
      i18n en/uk/ru.
- [ ] T005 Gates: `cargo test -p mixtape-yt`, `cargo check -p mixtape`, `cargo clippy -p mixtape-yt -p mixtape
      --all-targets -- -D warnings`, `pnpm check`, `pnpm test` in `ui/`.
