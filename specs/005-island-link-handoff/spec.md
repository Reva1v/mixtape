# Feature Specification: Hand Mixtape links to Droplet Island

**Feature Branch**: `005-island-link-handoff`
**Created**: 2026-11-26
**Status**: Draft
**Input**: Owner, 2026-11-26, after the browser extension worked live: «можно сделать что бы если остров открыт, то
трек запускался в нём?» The island side is agreed with the island session and gets its own spec in droplet-island.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A link plays in the island when the island is open (Priority: P1)

The owner keeps Droplet Island running all day; its Home tab has Mixtape built in. A friend sends a YouTube
Music link; the owner clicks it, the browser extension turns it into a Mixtape link, and the track plays in the
island's Mixtape. The separate Mixtape window doesn't open.

**Why this priority**: it is the whole request.

**Independent Test**: island running, Mixtape closed → open `mixtape://open?url=<YTM track link>` (Win+R) → the
island shows its Mixtape and plays the track; no Mixtape window or tray icon appears.

**Acceptance Scenarios**:

1. **Given** the island is running and Mixtape is not, **When** a Mixtape link is opened, **Then** the link goes to
   the island and Mixtape exits at once without showing a window.
2. **Given** the island and Mixtape both run, **When** a Mixtape link is opened, **Then** the island gets it (owner's
   choice) and Mixtape's own playback is not touched.
3. **Given** the island is running, **When** an album, playlist or artist link is opened, **Then** the island gets it
   the same way (what it shows is the island's part).

---

### User Story 2 - Without the island nothing changes (Priority: P1)

**Why this priority**: not everyone has the island; Mixtape alone must keep working exactly as in 0.6.0.

**Independent Test**: island closed → open a Mixtape link → Mixtape starts (or comes to the front) and plays, as today.

**Acceptance Scenarios**:

1. **Given** the island is not running, **When** a Mixtape link is opened, **Then** Mixtape handles it itself.
2. **Given** the island is running but handing over fails (its program cannot be started), **When** a link is
   opened, **Then** Mixtape handles it itself; the link is never lost.

---

### User Story 3 - The island can send a link back without a loop (Priority: P2)

When the island's music module is switched off, the island passes the link to Mixtape marked "from the island";
Mixtape then plays it itself and never hands it back.

**Independent Test**: start Mixtape with the link and the "from the island" mark while the island runs → Mixtape plays it.

**Acceptance Scenarios**:

1. **Given** the island is running, **When** Mixtape is started with a link marked "from the island", **Then**
   Mixtape handles the link itself.
2. **Given** a link in the explicit form (flag + link, the form the island uses), **When** Mixtape gets it, **Then**
   it is treated like the bare protocol form.

### Edge Cases

- Several links in a row while the island runs: each goes to the island, in order.
- A link that reaches an already running Mixtape (second launch) follows the same rule as a cold start.
- Another user's island on the same PC (fast user switching) does not count: only the current user's.
- A bad link (not `mixtape:`, longer than 4 KB): not handed over; Mixtape handles it as today (toast).
- Mixtape started without a link (autostart, Start menu) never looks for the island and never exits early.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: When Mixtape receives a Mixtape link (cold start or second launch) and the current user's island is
  running, it MUST pass the link to the island by starting the island's program (the path of the running island)
  with `--mixtape-link <link>` as two separate arguments.
- **FR-002**: After a successful hand-over on a cold start, Mixtape MUST exit without showing a window, a tray icon
  or making a sound, and without changing its saved state (queue, position, settings).
- **FR-003**: On a second launch the running Mixtape MUST hand the link over the same way and keep running as it
  was (no window raised, playback untouched).
- **FR-004**: If the island is not running, or starting it fails, Mixtape MUST handle the link itself exactly as in 0.6.0.
- **FR-005**: Mixtape MUST also accept the link as `--mixtape-link <link>`; with `--from-island` present it MUST
  handle the link itself and never hand it over.
- **FR-006**: Only links Mixtape itself would accept (scheme `mixtape:`, at most 4 KB) are handed over.
- **FR-007**: Starting Mixtape without a link MUST behave as today (no island lookup).
- **FR-008**: Nothing new is installed or registered: the `mixtape://` protocol still points at Mixtape.

### Key Entities

- **Mixtape link**: `mixtape://open?url=…` or `mixtape://play?ids=…` (spec 004).
- **Hand-over call**: the island program + `--mixtape-link <link>`; `--from-island` marks the way back.

## Success Criteria *(mandatory)*

- **SC-001**: With the island open, 10 of 10 links opened from the browser play in the island; the Mixtape window
  never appears.
- **SC-002**: With the island closed, links behave as in 0.6.0 (the spec 004 quickstart checks pass).
- **SC-003**: The hand-over adds under half a second before the island starts playing.
- **SC-004**: No loop: a link marked "from the island" is played by Mixtape on the first try.

## Assumptions

- "Island running" = a process named `droplet-island.exe` in the current user's session (the dev build has the same
  name); its program path is read from that process. Chosen over "always try the island when it is installed": a
  closed island shouldn't be started just to play a song, and Mixtape needn't know where the island is installed.
- The island validates the link again, shows its Home tab with Mixtape, and falls back to Mixtape with
  `--from-island` when its music module is off (island spec).
- The island already carries Mixtape 0.6.0's session with the link inbox, so the shared session crate needs no
  change for this feature.
- Windows only; no new dependencies.

## Out of Scope

- The island's handling (its own spec in droplet-island).
- A setting to turn the hand-over off (later, if the owner asks).
- The phone: it has no island.
