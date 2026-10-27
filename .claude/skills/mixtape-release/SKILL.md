---
name: mixtape-release
description: Release a new Mixtape version to GitHub (installer + in-app update). Use for «выпусти релиз», «release 0.2.3», «запушь релиз», «подними версию», or after work that should ship.
---

# Mixtape release

Every shipped change is a new version plus a GitHub release; installed copies update
themselves from `releases/latest/download/latest.json`. The agent runs the whole release,
including `scripts/release.ps1`.

## Steps

1. **Clean main.** `git switch main && git pull --ff-only`; `git status --short` is empty.
   Merge any branch the user named (e.g. `origin/mixtape-session`) with
   `git merge --no-ff origin/<branch>` and push. Done when `git log origin/main..main` and
   `git status --short` are both empty after the push.
2. **Green.** `cargo test -j 4 --workspace` and, in `ui/`, `pnpm check` and `pnpm test`.
   A red check stops the release: report the failure to the user.
3. **Version.** Pick the bump from what changed since the last tag
   (`git log $(git describe --tags --abbrev=0)..HEAD --oneline`):
   fixes → patch (0.2.2 → 0.2.3), noticeable features → minor (0.3.0), big change → major.
   Set it in `Cargo.toml` (`[workspace.package] version`) and `src-tauri/tauri.conf.json`,
   run `cargo check -j 4 -p mixtape` to refresh `Cargo.lock`, commit
   `chore: version X.Y.Z` (those three files), push main.
4. **Notes.** Write them to the scratchpad (never into the repo), English, in the shape of
   earlier releases (`gh release view v0.2.1 --repo Reva1v/mixtape`): `### Highlights`
   for what the user sees, `### Under the hood`, `### Install` naming
   `Mixtape_X.Y.Z_x64-setup.exe`, the update path from the previous version, and the
   SmartScreen «More info → Run anyway» line. Every bullet traces to a commit since the
   last tag.
5. **Publish.** From the repo root, in the background (the build takes ~10–20 min):
   ```powershell
   $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD=''; ./scripts/release.ps1 -Notes <notes.md>
   ```
   The updater key `.secrets/tauri-updater.key` has no password; the empty variable keeps
   the script from prompting. The script builds, signs, tags `vX.Y.Z`, pushes the tag and
   creates the release with the installer and `latest.json`.
6. **Verify.** Done when `gh release view vX.Y.Z --repo Reva1v/mixtape --json assets`
   lists both `Mixtape_X.Y.Z_x64-setup.exe` and `latest.json`, and
   `gh release list --repo Reva1v/mixtape -L 1` shows it as Latest. Report the version
   and the release URL.

## Gotchas

- Release from `D:\Projects\mixtape`. A worktree (e.g. `D:\Projects\mixtape-session`)
  has no `.secrets/`; release there only through a junction to the main repo's `.secrets`.
- One build at a time per target dir: check for a running `cargo build ... --release`
  in this repo before step 5.
- If the script fails after `git push origin <tag>`, delete the tag locally and on origin
  before rerunning.
