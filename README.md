<div align="center">

<img src="src-tauri/icons/128x128.png" width="96" height="96" alt="Mixtape">

# Mixtape

**A fast YouTube Music player for Windows, with your library on disk.**

Streams and downloads from your YouTube Music account, plays offline, and keeps mixes going like the web app does, in a light native window.

**English** · [Українська](README.uk.md) · [Русский](README.ru.md)

![Platform](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?logo=windows&logoColor=white)
![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-2024-CE422B?logo=rust&logoColor=white)
![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white)

</div>

---

<p align="center">
  <img src="docs/images/overview-en.png" width="860" alt="Mixtape home screen with the queue of a running mix and its filter chips">
</p>

<p align="center">
  <img src="docs/images/album-lyrics-en.png" width="860" alt="An album page next to the full-screen player with synced lyrics">
</p>

<sub>Screenshots use the built-in demo data: invented artists and generated covers.</sub>

## Features

| | |
|---|---|
| 🎧 **Your YouTube Music** | Sign in with Google in an app window; likes, albums, artists and playlists come over to the library. |
| 📻 **Mixes** | «Start mix» on any track builds an endless radio with the same filter chips as YouTube Music (Familiar, Discover…). |
| ⚡ **Instant streaming** | Tracks play while they download and the next ones are prepared ahead, so skipping doesn't wait. |
| 💾 **Offline library** | Download a track, album, playlist or a whole discography; tracks you play often are kept automatically. |
| 🎤 **Synced lyrics** | Line-by-line lyrics from LRCLIB in the full-screen player, with an offset control. |
| 🔎 **Search** | Your library and the YouTube Music catalog in one place, with suggestions and recent searches. |
| ⌨️ **Keyboard and Windows** | Keyboard shortcuts, media keys, the Windows media overlay and taskbar buttons. |
| 🌐 **Three languages** | English, Ukrainian and Russian, switched in Settings without a restart. |

## Usage

1. Open **Settings → YouTube Music account → Connect** and sign in with Google.
2. Pick what to bring over: liked songs, albums, artists, playlists. Progress shows on the **Downloads** screen.
3. Play anything; right-click a track (or the player bar) for **Start mix**, **Play next**, **Download**, **Like**.
4. On an artist page, **Load all songs** reads the whole discography into the library without downloading it.

| Shortcut | Action |
|---|---|
| `Space` · `K` | Play / pause |
| `Ctrl + →` / `Ctrl + ←` | Next / previous track |
| `→` / `←` | Seek 5 seconds |
| `↑` / `↓` | Volume ±5% |
| `S` · `R` · `L` | Shuffle · repeat · like |
| `F` · `Q` · `T` | Full-screen player · queue · lyrics |
| `Ctrl + K` · `/` | Search |
| `Ctrl + L` | Add from link (a pasted YouTube link offers a download too) |
| `?` | All shortcuts |

## Installation

Download `Mixtape_0.1.0_x64-setup.exe` from **[Releases](https://github.com/Reva1v/mixtape/releases/latest)** and run it. It installs for the current user (no admin rights) and speaks English, Ukrainian or Russian, following Windows. The installer is unsigned, so Windows SmartScreen shows «Windows protected your PC»; choose **More info → Run anyway**.

## Privacy

- YouTube cookies from the sign-in are encrypted with Windows DPAPI (`account.bin`) and are sent only to YouTube.
- Everything else stays in `%LOCALAPPDATA%\Mixtape`: the SQLite database, downloaded tracks, the streaming cache (5 GB by default, adjustable in **Settings → Storage**) and covers.
- Lyrics are looked up on LRCLIB by title, artist, album and duration.

## Building from source

Requirements: Windows 10/11 with WebView2 (preinstalled on Windows 11), [Rust](https://rustup.rs) 1.89+, [Node.js](https://nodejs.org) with [pnpm](https://pnpm.io).

```powershell
git clone https://github.com/Reva1v/mixtape.git
cd mixtape
pnpm install                       # Tauri CLI and the interface (pnpm workspace)
./scripts/fetch-sidecars.ps1       # yt-dlp and deno into src-tauri/binaries
pnpm tauri dev                     # run in development
pnpm tauri build                   # installer in target/release/bundle/nsis
```

## Architecture

The Rust core owns everything (library, player, network); the Svelte interface only draws it and talks to the core over Tauri IPC, with typed bindings generated from the Rust types.

```mermaid
flowchart LR
  UI["Svelte 5 UI<br/>WebView2"] <-->|IPC| App["Tauri app<br/>commands, events"]
  App --> Player["Player<br/>queue, mixes, prefetch"]
  Player --> Audio["Audio engine<br/>symphonia, cpal"]
  App --> Library["Library<br/>SQLite"]
  App --> Sync["Sync<br/>downloads, streaming cache"]
  Sync --> YT["YouTube Music<br/>InnerTube, yt-dlp"]
  Player --> Sync
```

| Layer | Technologies |
|---|---|
| Interface | Svelte 5, TypeScript, Tailwind CSS 4, Vite, motion, virtua |
| Shell | Tauri 2, WebView2 |
| Audio | symphonia + libopus, cpal (WASAPI), rubato |
| Data | SQLite (rusqlite), ts-rs bindings |
| Network | reqwest, YouTube Music InnerTube API, yt-dlp + deno sidecars, LRCLIB |
| Windows | `windows` crate: SMTC, taskbar buttons, DPAPI |

## Project layout

```
crates/core/       domain types shared by all crates and exported to TypeScript
crates/library/    SQLite library, search, library views, cache bookkeeping
crates/player/     queue, mixes, prefetching on top of the audio engine
crates/audio/      decoding, resampling, gapless and crossfaded output
crates/sync/       downloads, streaming into a sparse cache file, likes, lyrics
crates/yt/         everything that talks to YouTube Music
crates/artwork/    cover thumbnails and colour palettes
crates/win/        media overlay, taskbar buttons, DPAPI
src-tauri/         the app: commands, events, windows
ui/                the Svelte interface (ui/src/lib/i18n holds the translations)
specs/             feature specs, plan and status (Spec Kit)
scripts/           helper scripts (sidecar download)
```

## Development

```powershell
cargo test --workspace                       # Rust tests
cargo clippy --workspace --all-targets       # lints
pnpm -C ui check                             # svelte-check
pnpm -C ui dev                               # interface alone, open http://localhost:5173/?mock
```

`?mock` runs the interface in a browser on demo data; add `&lang=uk` and `&scene=album|library|fullscreen|queue` to open a given screen (this is how the screenshots are made). `MIXTAPE_DATA_DIR` points the app at another data folder.

## Documentation

- [`specs/001-mixtape-mvp/`](specs/001-mixtape-mvp) — specification, plan, tasks and [`STATUS.md`](specs/001-mixtape-mvp/STATUS.md) (in Russian).
