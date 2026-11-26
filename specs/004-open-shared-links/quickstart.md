# Quickstart: checking the feature

Tests stay headless (CLAUDE.md). Live checks that play sound run only with the owner's consent.

## Automated

```powershell
cargo test -p mixtape-sync url_import mixtape_link   # parser, find_link, share_url round trip, phone forms
cargo test -p mixtape-session link                    # SharedLink mapping, pending queue
cargo test -p mixtape links                           # argv → link extraction (src-tauri)
pnpm -C ui test                                       # search card logic, menu share items
pnpm -C ui check
node --test "tools/link-catcher-tests/*.test.js"      # the extension's match + "came from outside" rules
```

On Android (`../mixtape-android`): `cargo test` for `deeplink` (share text, https, `mixtape://open`).

## Manual, PC

1. Dev registration (once): `reg add HKCU\Software\Classes\mixtape /ve /d "URL:Mixtape" /f`,
   `reg add HKCU\Software\Classes\mixtape /v "URL Protocol" /d "" /f`,
   `reg add HKCU\Software\Classes\mixtape\shell\open\command /ve /d "\"<path>\mixtape-player.exe\" \"%1\"" /f`.
2. Search box: paste `https://music.youtube.com/watch?v=u1t22blUCnU&si=x` → «Трек по ссылке» card, title
   fills in; paste an album / playlist / artist link → card opens the page; type `two faced` → normal results.
3. Right-click a track → «Поделиться треком» → paste into Notepad: `https://music.youtube.com/watch?v=<id>`.
   Right-click an album card → «Поделиться альбомом». A local file: no item.
4. Win+R `mixtape://open?url=https%3A%2F%2Fmusic.youtube.com%2Fwatch%3Fv%3Du1t22blUCnU` with the app running,
   in the tray, and closed → the track plays, the window is in front. `mixtape://open?url=garbage` → toast.
5. Extension: load `tools/link-catcher` unpacked in Chrome and Edge; click the link in Telegram Desktop →
   first time: the prompt, tick «Always allow», press «Браузер больше не спрашивает»; second time: the tab
   closes on its own and the track plays. If «Always allow» isn't offered: build the R6 fallback.
6. Installer: install → `HKCU\Software\Classes\mixtape` exists; uninstall → gone.

## Manual, phone

1. Telegram → long-press a YTM link → Поделиться → Mixtape: plays (app closed and open).
2. Settings → «Открывать ссылки YouTube Music» → system screen → add the links → tap a link in Telegram → Mixtape.

## Live results, phone (2026-11-26, Samsung S23+, Android 16, Mixtape Android 0.3.0)

- **Share target works for text.** `ACTION_SEND text/plain` resolves to Mixtape and it shows in the system
  share sheet (checked with `am start -a android.intent.action.SEND -t text/plain`). AyuGram's «Share» on a
  message with media sends a non-text type, so Mixtape isn't listed there (Droplet is: it accepts `*/*`).
  To share a link, share the link itself, or paste it into search. Accepting `image/*` with the link in the
  caption was offered and not taken.
- **Direct links: music.youtube.com goes to Mixtape.** The owner's phone has no YouTube Music app, so
  nothing else claims the domain. Enabled with
  `pm set-app-links-user-selection --user 0 --package app.mixtape true music.youtube.com` (same switch as
  Settings → Apps → Mixtape → «Открывать по умолчанию»); `cmd package resolve-activity` now gives
  `app.mixtape/.MainActivity`.
- **youtube.com / youtu.be stay with the YouTube app.** Google's YouTube app holds them as `system_configured`,
  which beats a user selection. Taking them would mean turning off the YouTube app's links and sending every
  YouTube video to Mixtape; left as is.
