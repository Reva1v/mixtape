# Mixtape

## Disk budget

User rule 2026-10-28, after `target/` grew to ~50 GB: the repo with all caches stays at 15–20 GB,
30 GB is the hard cap. After a task that built anything (cargo, tauri, agent worktrees), run skill
`clean-cache` (dry run first); over 20 GB → clean, between tasks. A normal clean keeps the release
build's cache, so the next installer builds fast (user, 2026-11-03); `--full` only near the hard cap.

## Rust: best practices and fast agent work

- Stack rules: `~/.claude/toolbox/notes/best-practices.md`, section «11. Rust» (and «13. Tauri 2» for
  Tauri). This repo's own rules win over it.
- Speed without extra tokens (user 2026-11-09), same section, «Скорость разработки с агентами»: an agent
  checks and tests only its own crates (`cargo check -p` / `cargo test -p`), pure logic lives in light
  crates; the full workspace build, clippy and tests run once after the merge; Rust mutation checks and
  device/end-to-end tests (`#[ignore]`) run once after the merge too. 10–15 tasks per agent, bigger work
  goes to 2–3 agents on disjoint crates; no heavy build of your own while agents build.
- Tests are headless: nothing that opens windows, plays sound or records the screen/mic runs without the
  user's consent, and only when they are away from the machine.
