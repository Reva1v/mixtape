# Tasks — 003 yt-dlp update notice

- [x] T001 `crates/yt/src/ytdlp_release.rs`: `parse_version`, `is_newer`, `latest_tag` + unit tests (edge cases in plan).
- [x] T002 same file: `fetch_latest` (GitHub API, UA, timeout, body limit) and `installed_version` (sidecar `--version`)
      + fake-script test for `installed_version`.
- [x] T003 `src-tauri/src/updates.rs`: `spawn_ytdlp` loop, `ytdlp:available` event, `ytdlp_check` command, ts-rs types;
      register command (build.rs COMMANDS, capability), start the loop where `updates::spawn` starts.
- [x] T004 UI: store listens + toast once per version (localStorage), Settings → Updates line + check button,
      i18n en/uk/ru.
- [x] T005 Gates: `cargo test -p mixtape-yt`, `cargo check -p mixtape`, `cargo clippy -p mixtape-yt -p mixtape
      --all-targets -- -D warnings`, `pnpm check`, `pnpm test` in `ui/`.

## Auto-update addendum (auto-update.md)

- [x] T006 `crates/yt/src/ytdlp_update.rs`: age gate, URL allow-list, size caps, sums parsing, OpenPGP signature check
      with the embedded key (`crates/yt/keys/yt-dlp-public.key`), choose-which-exe, install/rollback file moves,
      bad-version list, rollback counter + unit tests (fixtures in `crates/yt/testdata`, made with gpg).
- [x] T007 `ytdlp_release.rs`: `Release`/`Asset` parse (`published_at`, assets); resolver: override exe, `probe`, batch hook.
- [x] T008 `src-tauri/src/updates.rs`: cycle (download, verify, smoke test, install), rollback on batch failures,
      `ytdlp_check` / `ytdlp_status` / `ytdlp_revert`, events `ytdlp:available|installed|rolledback`; pref `ytdlpAutoUpdate`.
- [x] T009 UI: Settings → Updates yt-dlp rows (status, check, «Вернуть встроенную», auto toggle), toasts, i18n en/uk/ru.
- [x] T010 Gates re-run on the merged result.
