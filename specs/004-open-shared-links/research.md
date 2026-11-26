# Research: Open shared YouTube Music links

## R1. How a link reaches the PC app

**Decision**: the `mixtape://` scheme is registered by the NSIS installer hooks under
`HKCU\Software\Classes\mixtape` (`URL Protocol`, `shell\open\command` = `"<exe>" "%1"`), and removed in
the uninstall hook. The URL comes in as a command-line argument: on a cold start from `std::env::args`,
on a second launch through the `args` of the single-instance callback that is already in `lib.rs`.

**Rationale**: the installer is already `currentUser`, and the single-instance plugin already forwards
argv. That covers both paths without a new dependency (constitution V).

**Alternatives**: `tauri-plugin-deep-link` does the same registration at runtime, plus `on_open_url`.
That's a new plugin for about 20 lines of code we would write anyway; rejected. Dev builds register by
hand (quickstart).

## R2. The `mixtape://` grammar

**Decision**: two forms, both case-insensitive in scheme and host:

- `mixtape://play?ids=<id>[,<id>…][&at=<ms>]`: the same form the phone already accepts from Droplet
  Island (`mixtape-android/src-tauri/src/deeplink.rs`). On the PC only the first id is used: it plays now.
- `mixtape://open?url=<percent-encoded URL>`: wraps any link `url_import::parse_link` understands. This
  is what the extension sends.

**Rationale**: one grammar on both devices, and a link built for one also works on the other.

## R3. One link parser

**Decision**: `crates/sync/src/url_import.rs` stays the single rule set. It gets three pure functions:
`find_link(text)` (the first recognised link inside free text, for the search box and phone shares),
`share_url(&Link)` (the music.youtube.com URL back), and a `mixtape_link` module for R2. Android
already depends on `mixtape-sync` through a git tag; after this merge it moves to a new tag and uses
`find_link` for shares and `mixtape://open`. Its own `link.rs` stays for its search box. A test on the PC
side pins the phone's accepted forms (SC-005).

**Alternatives**: duplicating the parser in TypeScript for the search card was rejected: two rule sets
drift apart, which is exactly the bug the owner reported.

## R4. Resolving a track link without the library

**Decision**: the new command `link_track(id)` calls `Yt::track` (InnerTube `next`, already used by the
import and doesn't need the account), stores the row the same way catalog search results are stored, and
returns `Track`. After that the existing `player_play` / `queue_add` work on the id as on any search
result.

## R5. What "play now" does with the queue

**Decision**:
- Search card: Играть = `player.play({kind:"tracks", trackIds:[id]})`, the same as clicking a track in
  catalog results. Далее / В очередь = `queueAdd(next|end)`.
- Handed-over link (protocol, extension): queue_add(next) then next. The current queue stays after it
  (spec US2-1).

## R6. Browser extension (Chrome + Edge, Manifest V3)

**Decision**: `tools/link-catcher/`, an unpacked MV3 extension. Permissions are `webNavigation`,
`storage`, and host permissions for `music.youtube.com`, `www.youtube.com`, `youtube.com`,
`m.youtube.com` and `youtu.be` only.
- **Superseded 2026-11-26** (bug `extension-misses-external-link`): the decision below raced a sleeping service worker; it is now made on `webNavigation.onCommitted` from `transitionType`/`transitionQualifiers` plus an in-memory per-tab "previous page was YouTube" note.
- `chrome.webNavigation.onBeforeNavigate` (frameId 0) is registered with URL filters
  (`hostSuffix: "youtube.com"`, `hostEquals: "youtu.be"`), so the extension never hears about other
  sites (FR-011). It checks the URL with a small copy of the recognition rules in JS (only to decide
  whether to catch; Mixtape parses it again). It then reads the tab's current page with
  `chrome.tabs.get(tabId)`: `tab.url` is visible only when that page is on one of the host permissions,
  that is, a YouTube site. A visible YouTube URL means the owner is browsing the site, so nothing is
  caught. No URL is undefined or blank, so the link came from outside and is caught. No browsing
  history is stored. YTM's own clicks are history updates, not navigations, so they never fire.
- A caught tab is sent to the extension page `handoff.html?url=…`. That page sets
  `location.href = "mixtape://open?url=…"`, shows «Открыто в Mixtape», and closes the tab after 1.5 s
  (or goes back if the tab has history). Until the owner presses «Браузер больше не спрашивает» once,
  it stays open, so the first-time «Open Mixtape?» prompt isn't killed by the close.
- Settings in a popup: «Перехватывать и обычный YouTube» (off), «Пауза» (off).

**Risk / fallback**: if the browser doesn't offer «Always allow» for an extension origin, the installer
writes the per-user policy `AutoLaunchProtocolsFromOrigins` for Chrome and Edge, allowing `mixtape` from
the extension's id. The id is pinned with a `key` in the manifest. Checked live in the quickstart before
this fallback is built.

**Alternatives**: `declarativeNetRequest` redirect rules can't target a custom scheme. A content script
runs only after the page started loading (slower, and the page flashes).

## R7. Shipping the extension

**Decision**: bundled as a resource (`link-catcher/`) and shown in Settings → Интеграции as
«Расширение для браузера»: a short explanation, «Открыть папку», and three install steps (developer
mode → «Загрузить распакованное» → the folder). The README gets the same steps.

## R8. Copy to clipboard

**Decision**: `navigator.clipboard.writeText` from the menu click (a user gesture in a secure context,
works in WebView2). No clipboard plugin. A failure shows an error toast.

## R9. Android: share target and direct links

**Decision**:
- Manifest: an `ACTION_SEND` `text/plain` filter. An `ACTION_VIEW` `https` filter for
  `music.youtube.com` (`/watch`, `/playlist`, `/browse`, `/channel`), `www.youtube.com` / `youtube.com` /
  `m.youtube.com` (`/watch`, `/shorts`) and `youtu.be`, with `autoVerify` off. The existing
  `mixtape://play` filter gains host `open`.
- `DeepLink.kt` turns every one of these into a string for Rust: the shared text, the https URL, or the
  mixtape link. It's handed over as today (`openLink`). Rust (`deeplink.rs`) tries `mixtape://play`
  first, then `mixtape://open`, then `url_import::find_link` on the raw text.
- Settings: «Открывать ссылки YouTube Music» shows `DomainVerificationManager` user state (API 31+) and
  opens `Settings.ACTION_APP_OPEN_BY_DEFAULT_SETTINGS`.

**Rationale**: reuses the hand-over channel built for Droplet Island, including cold-start buffering and
history-launch dedup.
