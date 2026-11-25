# Implementation Plan: Open shared YouTube Music links

**Branch**: `004-open-shared-links` | **Date**: 2026-11-25 | **Spec**: [spec.md](spec.md)

## Summary

There is one link parser, `mixtape_sync::url_import`, and it gains `find_link`, `share_url` and the
`mixtape://` grammar. On the PC that parser drives a link card in the search box, «Поделиться…» items in
the track and card menus, and handed-over links. Links come in through the `mixtape://` scheme, which
the installer registers, and reach the app through argv, using the existing single-instance plugin. A
small MV3 extension for Chrome and Edge turns YouTube Music navigations from outside into
`mixtape://open` links. On the phone, `../mixtape-android` moves to the new `mixtape-sync` tag and
routes shares and https links through its existing Droplet hand-over channel.

## Technical Context

**Language/Version**: Rust stable (2024), Svelte 5 + TypeScript, plain JS (extension), Kotlin (Android)

**Primary Dependencies**: existing only. Tauri 2, single-instance, `url`, NSIS hooks. No new crates or npm packages.

**Storage**: none (research: data-model.md)

**Testing**: `cargo test -p mixtape-sync/-p mixtape-session/-p mixtape`, vitest, `node --test` for the extension; no network

**Target Platform**: Windows 10/11 x64; Chrome/Edge (MV3); Android 8+ (direct links: 12+ settings screen)

**Project Type**: desktop app + browser extension + Android app (second repo)

**Performance Goals**: link card at once (local parse over IPC, no network before showing); handed-over track plays ≤ 3 s

**Constraints**: cold start budget (1.5 s) untouched, so argv parsing is trivial and the link waits for `app_ready`

**Scale/Scope**: about 15 files on the PC, 6 in Android, a new 5-file extension

## Constitution Check

| Principle | Check |
|---|---|
| I. Performance | Parsing is pure and microseconds; the network only when the card fills in or a link is acted on. Start-up unchanged. ✅ |
| II. UI as product | The card has loading, filled and failed states; toasts for share/failure; the extension gets a Settings entry with steps instead of a hidden folder. Keyboard: the card's buttons are focusable, Enter on the search box plays the link. ✅ |
| III. Local-first | Nothing new needs the network to run; resolving a shared track is a catalog read like search. ✅ |
| IV. YouTube behind one boundary | URL parsing lives in `sync::url_import` next to the existing import (URL text, not YouTube API). Track lookup goes through `Yt::track`. Nothing outside `yt` talks to InnerTube. ✅ |
| V. Simple personal app | No new dependency: the scheme is registered in the NSIS hooks, not with the deep-link plugin. The extension is a separate tiny artefact the owner chose, unpacked, no store. ✅ |
| VI. Clean-room | Nothing from Liner. ✅ |

Post-design re-check: same, no deviations.

## Project Structure

### Documentation (this feature)

```text
specs/004-open-shared-links/
├── spec.md  plan.md  research.md  data-model.md  quickstart.md
├── contracts/interfaces.md
└── checklists/requirements.md
```

### Source Code

```text
crates/sync/src/url_import.rs        + find_link, share_url, tests (incl. the phone's forms)
crates/sync/src/mixtape_link.rs      new: MixtapeLink parse / open_url
crates/session/src/commands/link.rs  new: link_parse, link_track, link_share_url, hand_over(url) / set_ready (pending FIFO, events)
crates/session/src/commands/mod.rs   register
src-tauri/src/links.rs               new: argv → links; cold start + single-instance callback → session::link::hand_over
src-tauri/src/lib.rs                 single-instance callback passes args; app_ready drains pending
src-tauri/installer-hooks.nsh        HKCU\Software\Classes\mixtape on install, removed on uninstall
src-tauri/tauri.conf.json            resource: ../tools/link-catcher → link-catcher/
ui/src/screens/Search.svelte         link card instead of library/catalog search when link_parse hits
ui/src/lib/ui/LinkCard.svelte        new: «Трек по ссылке» / album / playlist / artist card
ui/src/lib/ui/share.ts               new: copy share URL + toasts (used by both menus)
ui/src/lib/ui/trackMenu.ts           «Поделиться треком» (YTM id = 11-char video id)
ui/src/lib/ui/collectionMenu.ts      «Поделиться альбомом / плейлистом / исполнителем» (not local:)
ui/src/App.svelte                    listen link://route, link://failed
ui/src/lib/ui/IntegrationsSettings.svelte  «Расширение для браузера» block
ui/src/lib/i18n/{ru,uk,en}/…         new strings (skill i18n-keys)
tools/link-catcher/                  manifest.json, background.js, rules.js, handoff.html/js, popup.html/js (shipped)
tools/link-catcher-tests/            rules.test.js (node --test, not shipped)
ui/src/lib/ui/linkCard.svelte.ts     card state machine (idle → loading → ready | failed), pure, vitest
README*.md                           extension install steps

../mixtape-android/
src-tauri/Cargo.toml                 mixtape-* git tag → the new tag
src-tauri/gen/android/app/src/main/AndroidManifest.xml   SEND + https VIEW filters, host "open"
plugins/mixtape-media/.../DeepLink.kt  accept SEND/https/mixtape://open
src-tauri/src/deeplink.rs            mixtape_link + find_link; Open(Track) plays, pages → event to UI
src-tauri/src/links.rs + Kotlin      links_state / links_open_settings
src/… Settings + App                 settings item, route event
```

**Structure Decision**: the pure logic sits in light `mixtape-sync` (testable with `cargo test -p`, shared
with the phone). Session code holds the IPC and the side effects, `src-tauri` only the OS entry point.

## Order and agent split

1. `mixtape-sync` (find_link, share_url, mixtape_link) comes first: everything depends on it.
2. In parallel, on disjoint files: **A** session + src-tauri + NSIS (protocol, act_on, events);
   **B** UI (search card, share items, events, settings block, i18n); **C** `tools/link-catcher`.
3. After merge: full build, clippy, tests; tag; then **D** Android on the new tag.

## Complexity Tracking

None.
