# Bug Assessment: a click on a radio row plays another song, and radios are slow

- **Slug**: radio-wrong-track
- **Created**: 2026-11-03
- **Source**: pasted text and a screenshot from the user
- **Verdict**: valid
- **Severity**: high (a click plays the wrong song on every generated radio)

## Report

«радио в пк версии пофиксить, они тупят, так ещё и включается не та песня на которую нажимаешь, я нажал на 2 по счёту».
Screenshot: row 2 is RALLY FUNK; the mini player shows PULSE (Super Slowed).

## Symptom

A click on row N of a playlist outside the library plays a track that may not even be on screen.
Expected: exactly the clicked track, at once.

## Reproduction

1. Open a generated radio («Радио • …», id `RDAT…`).
2. Click any row.
3. Another track starts.

Measured with the radio from the app's `queue_snapshot`, two reads in a row, no client state involved:
`fetch 1: 727 ms, 100 tracks`; `fetch 2: 698 ms, 100 tracks`; 2 of 100 at the same position, second track differs.

## Suspected Code Paths

- `crates/session/src/commands/player.rs:70` — `play` reads a playlist outside the library from YouTube again and applies
  the index from the list on screen to the new answer.
- `crates/session/src/commands/library.rs:253` — `playlist_get` reads it from YouTube each time.
- `ui/src/screens/Playlist.svelte:59` — every `library:changed` calls `playlist_get`, so a radio on screen is replaced by a
  different list while the user looks at it.
- `crates/sync/src/catalog.rs:95` via `sources.rs:440` — every read waits for the covers of all its tracks, so each new
  order pays for new covers.

## Root Cause Hypothesis

Confidence: high for the wrong track, shown by the measurement above. A generated radio has no fixed contents, and the
list clicked and the list played are two separate reads. Confidence: medium for the slowness: a click costs a read (about
0.7 s) plus covers plus resolving the stream; the shares are not measured.

## Proposed Remediation

**Preferred**: one list per playlist. Keep the last read of a catalog playlist in the session; `play` uses it whatever its
age, without the network, and `playlist_get` reads again only when it is 30 minutes old. Reads of one list run one at a
time, so a slower older answer cannot replace the newer. Cleared when the account changes.

**Alternatives**:
- The UI sends the ids it shows (`PlaySource::Tracks`): loses the queue's playlist context and leaves the list changing on
  every `library:changed`.

**Files likely to change**:
- `crates/session/src/commands/catalog.rs`, `commands/player.rs`, `state.rs`, `account.rs`

**Tests to add or update**:
- A fetch answering a new order each time: the list and the play read get the same order; only stale reads fetch again;
  play ignores age; a cap on kept pages.

## Risks & Considerations

- The first opening of a radio still waits for its covers; not addressed here.
- A radio stays the same for up to 30 minutes when reopened. This is intended.

## Open Questions

- None blocking.
