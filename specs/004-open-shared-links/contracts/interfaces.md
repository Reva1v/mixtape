# Contracts

## Rust: `mixtape_sync::url_import` (pure, no network)

```rust
/// The first link `parse_link` recognises in free text: tokens split on whitespace and
/// `<>()"'«»`, trailing `.,;:!?` trimmed; a bare id counts only when it's the whole text.
pub fn find_link(text: &str) -> Option<Link>;

/// The music.youtube.com URL of a link:
/// Track → `https://music.youtube.com/watch?v=<id>`
/// Album → `https://music.youtube.com/browse/<MPREb id>`
/// Playlist → `https://music.youtube.com/playlist?list=<id without VL>`
/// Artist → `https://music.youtube.com/channel/<UC id>`
pub fn share_url(link: &Link) -> String;
```

Invariant (tested): `parse_link(&share_url(&l)) == Some(l)` for every kind.

## Rust: `mixtape_sync::mixtape_link`

```rust
pub enum MixtapeLink { Play { ids: Vec<TrackId>, at_ms: u64 }, Open(Link) }
pub fn parse(url: &str) -> Option<MixtapeLink>;
/// `mixtape://open?url=<percent-encoded share_url(link)>`
pub fn open_url(link: &Link) -> String;
```

## Tauri commands (PC, `mixtape_session::commands::link`)

| command | args | returns | note |
|---|---|---|---|
| `link_parse` | `text: String` | `Option<SharedLink>` | sync, no network; uses `find_link` |
| `link_track` | `id: TrackId` | `Track` | `Yt::track` + `sources::store_tracks`; errors map like catalog errors. The search card's buttons wait for it before playing/queueing |
| `link_share_url` | `kind, id` | `String` | `share_url`; the menus call it so the URL rules live in Rust |
| `extension_open_folder` | — | `()` | opens the bundled `link-catcher/` (dev: `tools/link-catcher`) in Explorer |

`pub fn hand_over(st, url)` / `pub fn set_ready(st)` (Rust, not commands): the pending FIFO for handed-over links.

## Events (PC, Rust → UI)

| event | payload | when |
|---|---|---|
| `link://route` | `SharedLink` (album/playlist/artist) | a handed-over page link; the UI calls `router.go` |
| `link://failed` | `string` (localised message) | a handed-over link couldn't be understood or the track couldn't load |

A handed-over track link is played in Rust (`link_track` → `queue_add(Next)` → `next`). The window is
shown and focused (`window::show_main`) for every handed-over link, including from the tray.

## Command line (PC)

`mixtape-player.exe "mixtape://…"`: any argument starting with `mixtape:` (case-insensitive) is a link;
other arguments (autostart's `--hidden`) behave as today.

## Browser extension → app

The tab navigates to `mixtape://open?url=<percent-encoded original URL>`. Nothing else is sent anywhere.

## Android

- Intent filters: see research R9.
- `DeepLink.handle(intent)` sends one string to Rust via `RustBridge.openLink`: the `ACTION_VIEW` data
  string, or `EXTRA_TEXT` of `ACTION_SEND`.
- `deeplink::open` order: `mixtape_link::parse` → else `url_import::find_link` → else the error event
  «В тексте нет ссылки на YouTube Music».
- New commands: `links_state() -> "allowed" | "partial" | "off" | "unsupported"` (API < 31 →
  `unsupported`), `links_open_settings()`.
