# Feature Specification: Open shared YouTube Music links in Mixtape

**Feature Branch**: `004-open-shared-links`

**Created**: 2026-11-25

**Status**: Draft

**Input**: User description: «почему такая логика есть на телефоне, но нет на ПК? … давай попробуем сделать какой-то перехватчик таких ссылок, хотя бы для ПК, хотя для телефона тоже было бы круто, чтобы я открыл трек с YouTube Music, который мне скинул друг, а оно у меня автоматом открывало его в моём Mixtape». The owner picked: browser extension + `mixtape://` protocol on the PC; share target + direct opening of links on the phone. Follow-up: add «Поделиться треком» to the PC track menu, as on the phone, copying the plain YouTube Music link, not a Mixtape one.

**Scope across repos**: stories 1–4 are built in this repo (PC). Stories 5–6 are built in `../mixtape-android` and tracked from this spec.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A pasted link in the PC search box (Priority: P1)

A friend sends `https://music.youtube.com/watch?v=u1t22blUCnU&si=…`. The owner pastes it into the PC search box. Today YouTube Music search gets the link as text and shows «Ничего не найдено». Instead, Mixtape recognises the link and shows a card at the top of the results, as the phone already does: «Трек по ссылке» with the track's title and artist once known, and the buttons Играть / Далее / В очередь. An album, playlist or artist link shows a card that opens that page.

**Why this priority**: closes the gap between the phone and the PC that the owner noticed, needs nothing installed, and is the base the other PC stories reuse.

**Independent Test**: paste each link form into the search box, check the card shows up and each button does its job.

**Acceptance Scenarios**:

1. **Given** the search box is empty, **When** the owner pastes a track link (music.youtube.com, youtube.com, m.youtube.com, youtu.be, shorts, with any extra parameters), **Then** a «Трек по ссылке» card appears above the results and plain-text search doesn't run.
2. **Given** the track card is shown, **When** the owner presses Играть / Далее / В очередь, **Then** the track plays now / goes next / goes to the end of the queue.
3. **Given** the owner pastes an album, playlist or artist link, **When** the card appears and they press it, **Then** that album, playlist or artist page opens.
4. **Given** the owner types ordinary text, **When** the text isn't a link, **Then** search works as before.
5. **Given** the card is shown, **When** the track's title and artist load, **Then** they replace the raw link on the card; if they can't load, the card keeps showing the link and Играть still works.

---

### User Story 2 - `mixtape://` links open in the app (Priority: P1)

Opening a `mixtape://` link anywhere in Windows (the Win+R box, a browser, a chat) brings up Mixtape: it starts if it's closed, comes to the front if it's running, and acts on the link. A track link starts playing the track. An album, playlist or artist link opens its page.

**Why this priority**: the browser extension needs this to hand links over, and it's useful on its own.

**Independent Test**: run `mixtape://play?v=<id>` from Win+R with the app closed and with it running.

**Acceptance Scenarios**:

1. **Given** Mixtape is running, **When** a `mixtape://` track link is opened, **Then** the window comes to the front and the track starts playing in place of the current one, the rest of the queue kept.
2. **Given** Mixtape is closed, **When** a `mixtape://` track link is opened, **Then** Mixtape starts and plays the track once it's ready.
3. **Given** a `mixtape://` link wraps a full YouTube Music URL, **When** it's opened, **Then** it's treated exactly like that URL pasted into search (story 1's link rules).
4. **Given** a `mixtape://` link is malformed or points at something unsupported, **When** it's opened, **Then** Mixtape comes to the front, shows a short notice («Не удалось открыть ссылку»), and nothing else changes.
5. **Given** the app was installed, **When** the owner looks at the protocol, **Then** it's registered for the current user by the installer and unregistered on uninstall.

---

### User Story 3 - Browser extension sends YouTube Music links to Mixtape (Priority: P2)

The owner installs a small extension in Chrome and Edge. A click on a YouTube Music link in Telegram opens the default browser; the extension catches the page before it loads, hands the link to Mixtape, and closes the tab (or goes back, if the link opened in an existing tab with history). The track plays in Mixtape.

**Why this priority**: this is the "it just opens in Mixtape" experience the owner asked for, but it depends on story 2 and needs a one-time install.

**Independent Test**: with the extension on, open a music.youtube.com track link from Telegram and check the track plays in Mixtape and the tab is gone.

**Acceptance Scenarios**:

1. **Given** the extension is on, **When** the browser navigates to a YouTube Music track, album, playlist or artist link, **Then** the link goes to Mixtape and the tab is closed or goes back.
2. **Given** the extension is on, **When** the browser opens a plain youtube.com or youtu.be link, **Then** by default nothing is caught; with the «Перехватывать и обычный YouTube» setting turned on, video links go to Mixtape too.
3. **Given** the owner is browsing music.youtube.com itself (clicking around inside the site), **When** they move between pages, **Then** the extension doesn't catch anything: it only catches navigations into a tab that isn't on a YouTube site already (a new tab from another app, a link from another site, an address typed in a non-YouTube tab). An address typed into a tab that is already on YouTube isn't caught: that's the owner browsing.
4. **Given** the owner wants to listen in the browser this once, **When** they press the extension's button or use its pause setting, **Then** catching is off until they turn it back on.
5. **Given** the extension hands a link over for the first time, **When** the browser asks whether to open Mixtape, **Then** the owner can tick «Always allow» and is not asked again.

---

### User Story 4 - «Поделиться треком» on the PC (Priority: P1)

The phone's track menu has «Поделиться»: it opens the system share sheet with `https://music.youtube.com/watch?v=<id>`. The PC menu has nothing like it (owner's screenshot of a track card menu: Включить микс … Перейти к исполнителю). A new item «Поделиться треком» in the PC track menu (right-click a track in any list or card, and the ⋯ menu) puts that same music.youtube.com link on the clipboard and shows «Ссылка скопирована». The link is always the YouTube Music one, never a `mixtape://` link (owner, 2026-11-25).

**Why this priority**: the owner asked for it directly; small and independent.

**Independent Test**: right-click a track, pick «Поделиться треком», paste into Notepad and into Mixtape's search box (story 1).

**Acceptance Scenarios**:

1. **Given** a track with a YouTube Music id, **When** the owner picks «Поделиться треком», **Then** the clipboard holds `https://music.youtube.com/watch?v=<id>` with no extra parameters and a notice confirms it.
2. **Given** a local file that has no YouTube Music id, **When** the owner opens its menu, **Then** the item isn't shown.
3. **Given** the clipboard can't be written, **When** the owner picks the item, **Then** a short error notice shows instead of the success one.
4. **Given** the owner right-clicks an album, playlist or artist card from YouTube Music (the owner's screenshot is the menu of a single's card), **When** they pick «Поделиться альбомом» / «Поделиться плейлистом» / «Поделиться исполнителем», **Then** its music.youtube.com link is copied the same way; a local playlist has no such item.

---

### User Story 5 - Share a link to Mixtape on the phone (Priority: P1 for the phone)

In Telegram (or any app) on Android the owner long-presses a YouTube Music link → «Поделиться» → Mixtape. Mixtape opens and plays the track (album, playlist, artist: opens the page). Works even with the YouTube Music app installed.

**Why this priority**: the phone path that always works, whatever other apps are installed.

**Independent Test**: share a track link from Telegram to Mixtape with the app closed and with it open.

**Acceptance Scenarios**:

1. **Given** shared text contains a YouTube link (alone or inside other text, e.g. «послушай https://youtu.be/… »), **When** it's shared to Mixtape, **Then** the first recognised link is opened.
2. **Given** shared text has no recognised link, **When** it's shared to Mixtape, **Then** Mixtape opens and shows «В тексте нет ссылки на YouTube Music».

---

### User Story 6 - Links open in Mixtape directly on the phone (Priority: P2 for the phone)

Tapping a music.youtube.com, youtube.com/watch or youtu.be link on the phone offers Mixtape as the app to open it, or opens Mixtape straight away once the owner allowed it in the system settings. A short guide in Mixtape's settings explains how to allow it and, if the YouTube Music app is installed, how to stop that app from taking the links.

**Why this priority**: the smoothest phone experience, but Android puts it behind manual settings, so it comes after sharing.

**Independent Test**: allow the links in the system settings, tap a link in Telegram, check the track plays in Mixtape.

**Acceptance Scenarios**:

1. **Given** the owner allowed Mixtape to open these links, **When** they tap a track link in another app, **Then** Mixtape opens and plays it.
2. **Given** the links aren't allowed yet, **When** the owner opens Mixtape's settings, **Then** a «Открывать ссылки YouTube Music» item shows the current state and opens the right system screen.

---

### Edge Cases

- A watch link that also carries a playlist (`watch?v=X&list=Y`) means the track, as in the existing «Добавить по ссылке» rules.
- Auto-generated mixes, the special `LL`/`LM` lists and `@handle` artist links aren't recognised; the search box falls back to plain search, a handed-over link shows «Не удалось открыть ссылку».
- A track that's unavailable (removed, region-locked) behaves like any other failed track: the existing playback error, no crash.
- Several links arrive quickly (the owner opens three in a row): each one acts in order; the last one is what plays.
- A link arrives while Mixtape is still starting: it's acted on once the app is ready, not lost.
- A link arrives while the owner isn't signed in to YouTube Music: track links still play (playback doesn't need the account today); pages that need the account behave as they do now.
- Text with leading/trailing spaces or newlines, links without `https://`, upper-case hosts: recognised.
- SoundCloud/Bandcamp links in the search box: out of scope here; they keep working in «Добавить по ссылке».

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The PC search box MUST recognise YouTube and YouTube Music links to tracks, albums, playlists and artists by the same rules as the existing «Добавить по ссылке» dialog, one rule set for both.
- **FR-002**: A recognised track link MUST show a card with Играть / Далее / В очередь above the results and MUST NOT send the link to catalogue search; a recognised album/playlist/artist link MUST show a card that opens its page.
- **FR-003**: The track card MUST fill in title, artist and cover when they can be loaded and MUST stay usable when they can't.
- **FR-004**: Mixtape on the PC MUST register the `mixtape://` protocol for the current user at install and remove it on uninstall.
- **FR-005**: Opening a `mixtape://` link MUST start Mixtape or bring the running window to the front, and MUST act on the link: a track plays now (current queue kept after it), an album/playlist/artist page opens.
- **FR-006**: A `mixtape://` link MUST accept both `mixtape://play?ids=<track id>[,…]` (the form the phone already takes from Droplet Island; the PC plays the first id) and `mixtape://open?url=<a wrapped YouTube/YouTube Music URL>`.
- **FR-007**: A link that arrives before the app is ready MUST be acted on once it's ready; several links MUST be acted on in arrival order.
- **FR-008**: A link that can't be understood MUST show «Не удалось открыть ссылку» and change nothing else.
- **FR-009**: The browser extension MUST work in Chrome and Edge, catch navigations to YouTube Music track/album/playlist/artist links that come from outside the site, hand them to Mixtape, and close the tab or go back.
- **FR-010**: The extension MUST leave plain YouTube links alone unless the owner turns on the setting to catch them, and MUST let the owner pause catching.
- **FR-011**: The extension MUST NOT read or send anything other than the link it's handing over, and MUST ask only for the permissions it needs for youtube.com, music.youtube.com and youtu.be.
- **FR-012**: The extension MUST ship with the app's source and be installable from a folder, with short install steps in the README.
- **FR-013**: The PC track menu MUST offer «Поделиться треком» wherever a track menu is shown, for tracks with a YouTube Music id; it MUST copy `https://music.youtube.com/watch?v=<id>` (the link the phone shares) and confirm with a notice; it MUST be hidden for local-only tracks. Album, playlist and artist card menus MUST offer the same for items from YouTube Music (album: `/browse/<MPREb id>`, playlist: `/playlist?list=<id>`, artist: `/channel/<UC id>`), and hide it for local playlists.
- **FR-014**: The Android app MUST accept shared text, find the first recognised YouTube link in it and act on it as in FR-005.
- **FR-015**: The Android app MUST declare itself able to open music.youtube.com, youtube.com/watch, m.youtube.com/watch and youtu.be links and MUST act on them as in FR-005.
- **FR-016**: The Android settings MUST show whether Mixtape may open these links and MUST open the system screen where the owner allows it.
- **FR-017**: Link recognition MUST be covered by tests with every supported form and the rejected ones, without network access.

### Key Entities

- **Shared link**: a piece of text that names one YouTube Music item; it resolves to a kind (track, album, playlist, artist) and an id, or to nothing.
- **Handed-over link**: a `mixtape://` link (PC) or an incoming share/link (phone) waiting to be acted on; kept until the app is ready.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A track link pasted into the PC search box shows its card at once; Играть starts the track as fast as starting any search result.
- **SC-002**: With the extension on, a YouTube Music track link clicked in Telegram on the PC starts playing in a running Mixtape within 3 seconds, with no browser tab left open and no other action from the owner.
- **SC-003**: With Mixtape closed, the same click starts the app and the track plays without a second click.
- **SC-004**: On the phone, sharing a link to Mixtape plays the track within 3 seconds of choosing Mixtape, whether the app was open or not.
- **SC-005**: Every link form the phone accepts today is also accepted on the PC (no form works on one and not the other).
- **SC-006**: Ordinary text searches on the PC return the same results as before this feature.

## Assumptions

- The owner is the only user; the extension is installed by hand from a folder (developer mode), not published to the Chrome or Edge stores.
- Windows can't hand music.youtube.com links straight to a desktop app without owning the domain, so the browser extension is the PC path; the owner's default browser is Chrome or Edge.
- Android 12+ won't verify music.youtube.com for Mixtape, so direct opening needs the owner to allow it once in the system settings; sharing works without that.
- Opening a track link means "play it now" (the default the owner described); Далее / В очередь are offered only on the search card.
- The PC and the phone keep separate link parsers in their own repos for now; they're held to the same set of accepted forms by tests (SC-005). Sharing one parser is a plan-time decision.
- The PC already runs as a single instance and brings its window forward when started again; the protocol builds on that.
- «Поделиться» on the PC copies to the clipboard rather than opening the Windows share window: the owner asked to copy, and pasting into Telegram is the usual next step.
- A `mixtape://` link is never what gets shared: friends without Mixtape couldn't open it (owner, 2026-11-25).
