# Contributing

Thanks for wanting to help. Bug reports, ideas and pull requests are welcome.

- **Bugs and ideas:** open an issue with steps to reproduce, the Mixtape version and Windows version.
  Security problems go through [SECURITY.md](SECURITY.md), not public issues.
- **Pull requests:** keep them focused on one change and describe what and why. For larger
  features, open an issue first so we can agree on the approach.

## Setup

See **Building from source** in the [README](README.md). Then, before sending a pull request:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm -C ui check
```

## Conventions

- Commit messages in English, in the style of the existing history: `feat(player): …`,
  `fix(sync): …`.
- Interface strings go through `t` from `$lib/i18n` and need all three languages (English,
  Ukrainian, Russian); English is the reference.
- New app commands must be added to `COMMANDS` in `src-tauri/build.rs` and to a capability in
  `src-tauri/capabilities/`, or the interface is not allowed to call them.
- Screenshots for docs come from the demo mock (`?mock&scene=…`), never from a real library.
- Tests must not play audio or open an audio device.

## License

By contributing you agree that your contribution is licensed under the
[GPL-3.0-or-later](LICENSE), like the rest of the project.
