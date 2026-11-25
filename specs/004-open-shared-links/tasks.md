# Tasks: Open shared YouTube Music links

**Input**: [spec.md](spec.md), [plan.md](plan.md), [research.md](research.md), [data-model.md](data-model.md), [contracts/interfaces.md](contracts/interfaces.md), [quickstart.md](quickstart.md)

**Tests**: required. Every new behaviour gets tests, edge cases included (owner's rule). No network in tests, nothing that opens windows or plays sound.

**Agents**: each agent runs `cargo check -p` / `cargo test -p` only for its own crates (CLAUDE.md). The full build, clippy and tests run once after the merge.

## Format: `[ID] [P?] [Story] Description`

## Phase 1: Setup

- [ ] T001 Create branch `004-open-shared-links` from `main`; agents work in worktrees off it (skill `parallel-agents`)

---

## Phase 2: Foundational (blocks every story)

Pure link logic in `mixtape-sync`, shared by the PC and the phone.

- [X] T002 Add `pub fn find_link(text: &str) -> Option<Link>` in `crates/sync/src/url_import.rs`: split on whitespace and `<>()"'«»`, trim trailing `.,;:!?`, return the first token `parse_link` accepts. A bare id/`PL…`/`MPREb_…`/`UC…` counts only when it is the whole trimmed text (so a random 11-letter word inside a sentence isn't a track)
- [X] T003 Add `pub fn share_url(link: &Link) -> String` in `crates/sync/src/url_import.rs`: Track → `https://music.youtube.com/watch?v=<id>`, Album → `https://music.youtube.com/browse/<id>`, Playlist → `https://music.youtube.com/playlist?list=<id>` (id without `VL`), Artist → `https://music.youtube.com/channel/<id>`
- [X] T004 Tests in `crates/sync/src/url_import.rs`: `find_link` (link alone, inside Russian text «послушай https://youtu.be/dQw4w9WgXcQ!», in `<…>` and «…», two links → first, no link, bare id inside a sentence → None, whole-text bare id → Some, upper-case host, no scheme); `share_url` round trip `parse_link(&share_url(&l)) == Some(l)` for all four kinds; SC-005: every form in `mixtape-android/src-tauri/src/link.rs` tests `accepts_every_link_form` (copy the list) parses to the same track, and the user's link `https://music.youtube.com/watch?v=u1t22blUCnU&si=bCH2wvLRATSlFy5M&drli=1`
- [X] T005 New `crates/sync/src/mixtape_link.rs` (export in `lib.rs`): `enum MixtapeLink { Play { ids: Vec<TrackId>, at_ms: u64 }, Open(Link) }`, `parse(url)` per research R2 (case-insensitive scheme/host, trailing `/` and `#…` ignored, `ids` percent-decoded and comma-split, invalid ids dropped, `Play` with no valid id → None, at most 500 ids, `open?url=` percent-decoded then `parse_link`), `open_url(&Link) -> String` = `mixtape://open?url=<percent-encoded share_url>`
- [X] T006 Tests in `crates/sync/src/mixtape_link.rs`: the phone's play forms (`at` missing/garbage → 0, `%2C` commas, repeats kept, 501 ids → 500), `open` with each link kind, `open_url` → `parse` round trip, `MIXTAPE://OPEN/?url=…`, wrong scheme, unknown host, `open` with garbage url, empty `ids` → None
- [X] T007 `cargo test -p mixtape-sync` and `cargo clippy -p mixtape-sync -- -D warnings` green

**Checkpoint**: the parser API is fixed; stories can start in parallel.

---

## Phase 3: User Story 1 — pasted link in the PC search box (P1) 🎯 MVP

**Goal**: a link in the search box shows a «Трек по ссылке» (or album/playlist/artist) card instead of «Ничего не найдено».
**Independent test**: quickstart, PC step 2.

- [ ] T008 [US1] New `crates/session/src/commands/link.rs`: `#[derive(Serialize, TS)] #[ts(export)] struct SharedLink { kind: SharedKind /* "track"|"album"|"playlist"|"artist" */, id: String }` with `From<&Link>`. The playlist id is in the form the UI `playlist` route and `catalog_playlist` expect: check how `catalog.rs` / the router build playlist ids (`VL` or not) and match it; `OLAK5uy_…` → kind `playlist`
- [ ] T009 [US1] Command `link_parse(text: String) -> Option<SharedLink>` (sync, `find_link`) in `crates/session/src/commands/link.rs`; register in `crates/session/src/lib.rs` `generate_handler!` and `commands/mod.rs`
- [ ] T010 [US1] Command `link_track(id: TrackId) -> CmdResult<Track>` in `crates/session/src/commands/link.rs`: `st.yt.track(&id)` → `sources::store_tracks(st.yt.http(), &st.db, &st.covers, vec![t])` → the stored `Track`; map errors as catalog commands do (`search_error`/`yt_error`); no account needed
- [ ] T011 [US1] Tests in `crates/session/src/commands/link.rs`: `SharedLink` mapping for all kinds incl. `OLAK5uy_` and the playlist-id form; `link_parse` on text with/without a link
- [ ] T012 [US1] Regenerate ts-rs bindings (`SharedLink`, `SharedKind`) into `ui/src/lib/ipc/bindings/` the way the repo does it (see how other `#[ts(export)]` types are exported)
- [X] T013 [P] [US1] New `ui/src/lib/ui/LinkCard.svelte`: props `link: SharedLink`. Track: icon + «Трек по ссылке» + the raw link until `link_track` answers, then cover/title/artists; buttons Играть (`player.play({kind:"tracks", trackIds:[id]})`), Далее / В очередь (`player.queueAdd([id], "next"|"end")` + the existing toasts). Every button first awaits the card's `link_track` promise (the track row must exist before the player gets the id); a pressed button shows a spinner while it waits. On `link_track` error the card keeps the raw link, shows «Не удалось загрузить данные трека», and the buttons try the action once anyway with the bare id; if that fails too, the error toast. State lives in `ui/src/lib/ui/linkCard.svelte.ts` (T016a) Album/playlist/artist: a card «Альбом по ссылке» / «Плейлист по ссылке» / «Исполнитель по ссылке», click → `router.go`. Visual style as the phone screenshot and the PC's catalog top card; buttons keyboard-focusable
- [X] T014 [US1] `ui/src/screens/Search.svelte`: on query change call `link_parse` (only when the text contains `youtu` or looks like a bare id, to skip IPC for ordinary typing); when it hits, render `LinkCard` and skip `library_search` and `CatalogResults`; Enter in the input plays a track link; plain text unchanged
- [X] T015 [US1] Strings ru/uk/en for the card in `ui/src/lib/i18n/*/` (skill `i18n-keys`)
- [X] T016a [US1] `ui/src/lib/ui/linkCard.svelte.ts` + `linkCard.test.ts`: the card's state as a pure class with the IPC passed in: `idle → loading → ready(track) | failed(msg)`; `act(fn)` awaits loading, runs `fn`, retries once with the bare id after a failure; Enter → play. Vitest: success, `link_track` failure then act succeeds, both fail → error, act pressed during loading waits, a new link replaces the old one and the late answer of the old one is ignored
- [X] T016 [US1] Vitest for the "should we ask Rust" pre-check (move it into a small pure helper, e.g. `ui/src/lib/ipc/linkHint.ts` + `.test.ts`): links of every host, bare id, ordinary words, empty

**Checkpoint**: US1 works alone.

---

## Phase 4: User Story 4 — «Поделиться» in PC menus (P1)

**Goal**: track/album/playlist/artist menus copy the YouTube Music link.
**Independent test**: quickstart, PC step 3.

- [ ] T017 [P] [US4] Command `link_share_url(kind: SharedKind, id: String) -> CmdResult<String>` in `crates/session/src/commands/link.rs` (builds `Link`, `share_url`); reject a local id (`local…`, `sc…`, `bc…`, `local:` playlists) with `ErrorKind::Invalid`; tests for each kind and the rejections
- [X] T018 [P] [US4] New `ui/src/lib/ui/share.ts`: `isYtmTrack(id)` = `/^[A-Za-z0-9_-]{11}$/`, `isYtmPlaylist(id)` = not `local:`; `share(kind, id)` → `link_share_url` → `navigator.clipboard.writeText` → toast «Ссылка скопирована», error toast on failure; vitest `share.test.ts` for the two predicates (YTM ids, `local0000000001`, `sc` + 16 hex, `local:uuid`, `VL…`, `PL…`)
- [X] T019 [US4] `ui/src/lib/ui/trackMenu.ts`: «Поделиться треком» (lucide/solar share icon, as other icons) after «Перейти к исполнителю», only when `isYtmTrack(track.id)`
- [X] T020 [US4] `ui/src/lib/ui/collectionMenu.ts`: «Поделиться альбомом» in `albumMenu`, «Поделиться исполнителем» in `artistMenu`, «Поделиться плейлистом» in `playlistMenu` (hidden for local playlists), in the last group
- [X] T021 [US4] Strings ru/uk/en (`i18n-keys`): the four labels, «Ссылка скопирована», «Не удалось скопировать ссылку»

---

## Phase 5: User Story 2 — `mixtape://` links open in the app (P1)

**Goal**: `mixtape://play?ids=…` and `mixtape://open?url=…` start/bring up Mixtape and act.
**Independent test**: quickstart, PC steps 1, 4, 6.

- [ ] T022 [US2] In `crates/session/src/commands/link.rs`: `pub fn hand_over(st: &Arc<Session>, url: String)` with a pending FIFO (`Mutex<VecDeque<String>>` + `ready: AtomicBool` on a static or in `Session`). Before ready → push; `pub fn set_ready(st)` drains in order. Acting on one link: `mixtape_link::parse`, else `url_import::find_link` on the raw string (a plain https link passed on the command line works too); Track (or `Play` first id) → `link_track` logic → `queue_add(Next)` then `next()` on the player (current queue stays); Album/Playlist/Artist → emit `link://route` with `SharedLink`; nothing → emit `link://failed` with localised «Не удалось открыть ссылку» (`tr(en, uk, ru)`); track load error → `link://failed` with the error text. Links are acted on one after another (a single tokio task / mutex), so the last one is what plays
- [ ] T023 [US2] Tests for `hand_over` ordering and buffering in `crates/session/src/commands/link.rs` with the acting step behind a small trait/closure so the test needs no player or network: before ready nothing runs; `set_ready` runs them in arrival order; after ready runs at once; garbage → failed path
- [ ] T024 [US2] New `src-tauri/src/links.rs`: `fn links_in(args: impl Iterator<Item=String>) -> Vec<String>` = arguments starting with `mixtape:` (case-insensitive); tests (none, one, with `--hidden`, upper-case, a path argument)
- [ ] T025 [US2] `src-tauri/src/lib.rs`: single-instance callback → `window::show_main(app)` then for each `links_in(args)` → `hand_over`; in `setup` → `links_in(std::env::args().skip(1))` → `hand_over` (buffered); a start that carries a link always shows the window, even when it was launched by autostart with `--hidden` (a link counts as a non-hidden start)
- [ ] T026 [US2] `src-tauri/src/window.rs` `app_ready`: call `set_ready` after the window is shown (and also in the tray branch, after `show_main` if links are pending)
- [X] T027 [US2] `ui/src/App.svelte`: listen `link://route` → `router.go({name: kind, id})`, `link://failed` → error toast
- [X] T028 [US2] `src-tauri/installer-hooks.nsh`: in `NSIS_HOOK_POSTINSTALL` write `HKCU\Software\Classes\mixtape` (default `URL:Mixtape`, `URL Protocol` = ``, `DefaultIcon` = `"$INSTDIR\${MAINBINARYNAME}.exe",0`, `shell\open\command` = `"$INSTDIR\${MAINBINARYNAME}.exe" "%1"`); add `NSIS_HOOK_POSTUNINSTALL` (or PREUNINSTALL) that `DeleteRegKey HKCU "Software\Classes\mixtape"`. Check the macro and variable names Tauri's NSIS template provides (`MAINBINARYNAME`, `$INSTDIR`) in the Tauri bundler template before writing
- [X] T029 [US2] Strings: «Не удалось открыть ссылку» in Rust `tr(...)` (no UI i18n needed)

---

## Phase 6: User Story 3 — browser extension (P2)

**Goal**: a YouTube Music link opened from outside goes to Mixtape, the tab closes.
**Independent test**: quickstart, PC step 5.

- [X] T030 [P] [US3] `tools/link-catcher/manifest.json`: MV3, name «Mixtape Link Catcher», `permissions: ["webNavigation","storage"]`, `host_permissions` for `*://music.youtube.com/*`, `*://www.youtube.com/*`, `*://youtube.com/*`, `*://m.youtube.com/*`, `*://youtu.be/*`, `background.service_worker: background.js` (`type: module`), `action.default_popup: popup.html`, icons from `src-tauri/icons`, a fixed `key` so the extension id is stable (generate once, note the id in the README)
- [X] T031 [P] [US3] `tools/link-catcher/rules.js` (ES module, no deps): `catchable(url, {catchYoutube})` → true for music.youtube.com `watch?v=`, `playlist?list=`, `browse/MPREb_…`, `browse/VL…`, `channel/UC…`; with `catchYoutube` also youtube.com/m.youtube.com `watch?v=`, `shorts/`, and `youtu.be/<id>`; `fromOutside(currentTabUrl)` → true when it is undefined/empty/`about:blank`/`chrome://newtab`/`edge://newtab` or its host isn't a YouTube host; `handoffUrl(url)` = `mixtape://open?url=` + `encodeURIComponent(url)`
- [X] T032 [US3] `tools/link-catcher-tests/rules.test.js` (`node --test "tools/link-catcher-tests/*.test.js"`, imports `../link-catcher/rules.js`; kept out of the shipped folder): every catchable form, plain YouTube with the setting off/on, a YTM non-content page (`music.youtube.com/`, `/library`), in-site navigation not caught, new tab caught, a typed URL caught
- [X] T033 [US3] `tools/link-catcher/background.js`: `chrome.webNavigation.onBeforeNavigate.addListener(cb, {url: [{hostSuffix: "youtube.com"}, {hostEquals: "youtu.be"}]})`, so no other site's navigation ever reaches the extension (FR-011); no history is tracked or stored. In `cb` (frameId 0): if not paused and `catchable`, read `(await chrome.tabs.get(tabId)).url`. With host permissions only for YouTube hosts it is defined only when the tab is on YouTube now. Then `fromOutside(thatUrl)` → `chrome.tabs.update(tabId, {url: chrome.runtime.getURL("handoff.html?url=" + encodeURIComponent(url))})`; also update the action badge when paused
- [X] T034 [US3] `tools/link-catcher/handoff.html` + `handoff.js`: on load set `location.href = handoffUrl(url)` via a hidden `<a>` click or `window.location`; show «Открыто в Mixtape» and the original link with «Открыть в браузере» (pauses catching for that one navigation, then goes there); if `promptAllowed` → after 1.5 s close the tab (`chrome.tabs.remove`, or `history.back()` when the tab has history before the caught page); else show the first-run hint «Отметьте "Всегда разрешать" в запросе браузера» and a button «Браузер больше не спрашивает» that sets `promptAllowed`. Dark style matching Mixtape
- [X] T035 [US3] `tools/link-catcher/popup.html` + `popup.js`: switches «Перехватывать и обычный YouTube» (`catchYoutube`, off) and «Пауза» (`paused`, off), stored in `chrome.storage.local`
- [X] T036 [US3] Ship the extension: `src-tauri/tauri.conf.json` `bundle.resources` += `"../tools/link-catcher/": "link-catcher/"` (the folder holds no tests, see T032)
- [ ] T036a [US3] Session command `extension_open_folder()` in `crates/session/src/commands/link.rs` (or the host trait if resource paths live in `src-tauri`): opens `resource_dir()/link-catcher` in Explorer, dev builds `tools/link-catcher` in the repo; register it
- [X] T037 [US3] `ui/src/lib/ui/IntegrationsSettings.svelte`: block «Расширение для браузера» with one-line purpose, «Открыть папку», and the three steps (open `chrome://extensions` / `edge://extensions`, turn on developer mode, «Загрузить распакованное» → the folder); strings ru/uk/en (`i18n-keys`)
- [ ] T038 [US3] Live check of «Always allow» for the extension origin in Chrome and Edge (quickstart step 5). Only if it's missing: in `installer-hooks.nsh` write `HKCU\Software\Policies\Google\Chrome\AutoLaunchProtocolsFromOrigins` and the Edge equivalent as JSON `[{"protocol":"mixtape","allowed_origins":["chrome-extension://<id>"]}]`, removed on uninstall; record the outcome in research.md R6

---

## Phase 7: User Story 5 — share to Mixtape on the phone (P1 for the phone), repo `../mixtape-android`

Starts after the PC branch is merged and tagged (T050).

- [ ] T039 [US5] `../mixtape-android/src-tauri/Cargo.toml`: every `mixtape-*` git dep → the new tag from T050; `cargo check` (Android target per that repo's docs) passes
- [ ] T040 [US5] `../mixtape-android/src-tauri/gen/android/app/src/main/AndroidManifest.xml`: on the main activity add `<intent-filter>` `ACTION_SEND` + `DEFAULT` + `mimeType="text/plain"`; add `<data android:host="open"/>` to the existing `mixtape` filter
- [ ] T041 [US5] `../mixtape-android/src-tauri/plugins/mixtape-media/android/src/main/java/app/mixtape/media/DeepLink.kt`: accept `ACTION_VIEW` with scheme `mixtape` (hosts `play`, `open`) or `https` (the hosts of T045), and `ACTION_SEND` with `EXTRA_TEXT`; send that one string to `RustBridge.openLink`; keep the handled-extra and history-launch guards for every kind
- [ ] T042 [US5] `../mixtape-android/src-tauri/src/deeplink.rs`: `open` tries the existing `parse` (play), then `mixtape_sync::mixtape_link::parse` (`Open`), then `mixtape_sync::url_import::find_link`; Track → plays now as the existing single-id play does; Album/Playlist/Artist → emit an event the UI turns into `nav` `{kind, id}` (match the id forms the phone's `nav.svelte.ts` uses); none → the error event «В тексте нет ссылки на YouTube Music» (`tr` ru/uk/en)
- [ ] T043 [US5] Tests in `../mixtape-android/src-tauri/src/deeplink.rs`: classification of shared text (link alone, inside text, two links, no link), https album/playlist/artist, `mixtape://open`, the old `mixtape://play` still wins
- [ ] T044 [US5] `../mixtape-android/src/App.svelte` (or where `player://error` is listened): listen the new route event → open the sheet/screen for album/playlist/artist

---

## Phase 8: User Story 6 — links open in Mixtape directly on the phone (P2), repo `../mixtape-android`

- [ ] T045 [US6] Manifest: an `ACTION_VIEW` + `DEFAULT` + `BROWSABLE` filter, scheme `https`, hosts `music.youtube.com` (path prefixes `/watch`, `/playlist`, `/browse`, `/channel`), `www.youtube.com`, `youtube.com`, `m.youtube.com` (`/watch`, `/shorts`), `youtu.be` (any path); no `autoVerify`
- [ ] T046 [US6] Kotlin in the media plugin: `linksState()` → `allowed` / `partial` / `off` from `DomainVerificationManager.getDomainVerificationUserState(packageName).hostToStateMap` (SELECTED/VERIFIED counts over the declared hosts), `unsupported` below API 31; `openLinksSettings()` → `Settings.ACTION_APP_OPEN_BY_DEFAULT_SETTINGS` with `package:` URI (fallback `ACTION_APPLICATION_DETAILS_SETTINGS`)
- [ ] T047 [US6] Rust commands `links_state`, `links_open_settings` in `../mixtape-android/src-tauri/src/links.rs` via `run_mobile_plugin` (as `share` does in `commands.rs`); register; a test for the state string mapping
- [ ] T048 [US6] Settings item «Открывать ссылки YouTube Music» in the phone's settings screen: state text (Включено / Частично / Выключено), tap → system screen, refresh state on resume; short hint about turning off «Открывать поддерживаемые ссылки» in YouTube Music; strings ru/uk/en

---

## Phase 9: Polish & cross-cutting

- [ ] T049 Merge the agents' branches into `004-open-shared-links`; full `cargo build`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `pnpm -C ui check`, `pnpm -C ui test`, `node --test "tools/link-catcher-tests/*.test.js"` (one run, after the merge)
- [ ] T050 Merge to `main`, then tag for Android deps per `mixtape-android` memory (tag form `v<version>-android.<n>`, next after `v0.3.1-android.18`), push the tag
- [ ] T051 [P] README.md / README.ru.md / README.uk.md: «Открывать ссылки в Mixtape»: the search box, «Поделиться», the extension install steps
- [ ] T052 Mutation check of the new Rust parsers (`find_link`, `share_url`, `mixtape_link::parse`, `links_in`) with skill `mutation-check`, once after the merge
- [ ] T053 Owner's live checks (quickstart, PC and phone), only with consent and when sound is OK; then release per `mixtape-release` / phone release script
- [ ] T054 Update `specs/001-mixtape-mvp/STATUS.md` and run `clean-cache` (dry run first)

---

## Dependencies

- Phase 2 (T002–T007) blocks everything.
- US1 (T008–T016) and US4 (T017–T021) share `commands/link.rs`: same agent, US1 first.
- US2 (T022–T029) needs `link_track` logic from T010 (same file): same agent as US1/US4, or after it.
- US3 (T030–T038) is independent of the Rust side except the bundle line (T036) and the Settings block (T037); the protocol (US2) is needed only for the live check.
- US5/US6 (T039–T048) wait for T050.

## Parallel plan

- Agent **A** (`sonnet-5.5-high`): T008–T012, T017, T022–T026, T028–T029, T036a (session + src-tauri + NSIS).
- Agent **B** (`sonnet-5.5-high`): T013–T016a, T018–T021, T027, T037 (UI). Starts once T012's bindings exist, or writes against the contract types and regenerates after.
- Agent **C** (`sonnet-5.5-medium`): T030–T036 (extension + bundle line).
- Agent **D** (`sonnet-5.5-high`, after T050): T039–T048 (Android).

## Implementation strategy

MVP = Phase 2 + US1 + US4 (the two things the owner noticed missing), shippable on its own. Then US2 + US3 (link catching on the PC), then US5 + US6 on the phone.
