# Bug Fix: a click on a radio row plays another song, and radios are slow

- **Slug**: radio-wrong-track
- **Fixed**: 2026-11-03
- **Assessment**: ./assessment.md
- **Status**: applied

## Summary

The list on screen and the list a click plays were two reads of a radio that answers differently on each read. They are
now one read, kept in the session.

## Changes

| File | Change | Notes |
|------|--------|-------|
| `crates/session/src/commands/catalog.rs` | modified | `CatalogPages`: one kept read per playlist, one read at a time |
| `crates/session/src/commands/player.rs` | modified | a click plays the kept read, without the network |
| `crates/session/src/state.rs` | modified | holds the pages |
| `crates/session/src/account.rs` | modified | pages are cleared when the account changes |

## Tests Added or Updated

- `pages_tests::the_list_clicked_is_the_list_played` — a read answering a new order each time; one read in all.
- `pages_tests::an_old_list_is_read_again_only_for_the_screen`, `a_failed_read_keeps_nothing`, `keeps_a_limited_number_of_lists`.

## Local Verification

- `cargo test -p mixtape-session --lib` → 53 passed.
- Live, before the fix: two reads of one radio share 2 of 100 positions (assessment). Not yet clicked in the built app.

## Deviations from Assessment

None.

## Follow-ups

- The first opening of a radio still waits for all its covers. Showing the list first would remove that wait.
