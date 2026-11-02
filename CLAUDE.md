# Mixtape

## Disk budget

User rule 2026-10-28, after `target/` grew to ~50 GB: the repo with all caches stays at 15–20 GB,
30 GB is the hard cap. After a task that built anything (cargo, tauri, agent worktrees), run skill
`clean-cache` (dry run first); over 20 GB → clean, between tasks. A normal clean keeps the release
build's cache, so the next installer builds fast (user, 2026-11-03); `--full` only near the hard cap.
