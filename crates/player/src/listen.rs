//! Real listening time of each play, for the statistics log (`play_events`).
//!
//! Time is counted from the engine's position reports, which follow the frames the
//! device actually played: pauses, buffering and seeks add nothing. A play is logged when
//! its track stops being audible (ended, skipped, replaced, failed) after at least
//! [`MIN_LISTEN_MS`] of listening.

use mixtape_core::TrackId;

/// Shorter plays are not logged.
pub const MIN_LISTEN_MS: u64 = 30_000;
/// Positions arrive about once a second while playing; a bigger step is a jump.
const MAX_STEP_MS: u64 = 5_000;
/// The last stretch of a play is reported with its end, after up to a crossfade (≤ 12 s)
/// with no position reports for it.
const MAX_END_STEP_MS: u64 = 15_000;
/// Plays followed at once: the audible one and one fading out under it.
const MAX_OPEN: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listen {
    pub track_id: TrackId,
    /// When the play became audible (Unix ms).
    pub started_at: i64,
    pub listened_ms: u64,
}

#[derive(Debug)]
struct Open {
    listen: Listen,
    /// The last position heard for this play.
    last_ms: u64,
}

impl Open {
    fn advance(&mut self, ms: u64, max_step: u64) {
        if ms >= self.last_ms && ms - self.last_ms <= max_step {
            self.listen.listened_ms += ms - self.last_ms;
        }
        self.last_ms = ms;
    }
}

fn qualifies(l: Listen) -> Option<Listen> {
    (l.listened_ms >= MIN_LISTEN_MS).then_some(l)
}

/// Plays in progress, oldest first.
#[derive(Debug, Default)]
pub struct Listens {
    open: Vec<Open>,
}

impl Listens {
    /// `id` became audible at `position_ms`, at wall-clock `now`. Returns plays closed to
    /// make room that are long enough to log.
    pub fn start(&mut self, id: TrackId, position_ms: u64, now: i64) -> Vec<Listen> {
        let mut closed = Vec::new();
        while self.open.len() >= MAX_OPEN {
            closed.extend(qualifies(self.open.remove(0).listen));
        }
        self.open.push(Open {
            listen: Listen {
                track_id: id,
                started_at: now,
                listened_ms: 0,
            },
            last_ms: position_ms,
        });
        closed
    }

    /// The newest play of `id` (a repeated track's new play starts before the old one ends).
    fn newest(&mut self, id: &str) -> Option<&mut Open> {
        self.open
            .iter_mut()
            .rev()
            .find(|o| o.listen.track_id.as_str() == id)
    }

    /// A position report: counts what played since the previous one.
    pub fn position(&mut self, id: &str, ms: u64) {
        if let Some(o) = self.newest(id) {
            o.advance(ms, MAX_STEP_MS);
        }
    }

    /// The user moved the playhead: the jump is not listening.
    pub fn seek(&mut self, id: &str, ms: u64) {
        if let Some(o) = self.newest(id) {
            o.last_ms = ms;
        }
    }

    /// The oldest play of `id` stopped, at `at_ms` when known. Returns it if long enough
    /// to log.
    pub fn end(&mut self, id: &str, at_ms: Option<u64>) -> Option<Listen> {
        let i = self
            .open
            .iter()
            .position(|o| o.listen.track_id.as_str() == id)?;
        let mut o = self.open.remove(i);
        if let Some(ms) = at_ms {
            o.advance(ms, MAX_END_STEP_MS);
        }
        qualifies(o.listen)
    }

    /// Closes everything, e.g. when the app quits.
    pub fn finish(&mut self) -> Vec<Listen> {
        self.open
            .drain(..)
            .filter_map(|o| qualifies(o.listen))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> TrackId {
        TrackId::from(s)
    }

    /// Reports a position every second from `from` to `to` (inclusive), in seconds.
    fn play(l: &mut Listens, track: &str, from: u64, to: u64) {
        for s in from..=to {
            l.position(track, s * 1000);
        }
    }

    #[test]
    fn counts_played_time_and_logs_at_the_end() {
        let mut l = Listens::default();
        assert!(l.start(id("a"), 0, 1_000).is_empty());
        play(&mut l, "a", 0, 40);
        let done = l.end("a", Some(40_500)).unwrap();
        assert_eq!(
            done,
            Listen {
                track_id: id("a"),
                started_at: 1_000,
                listened_ms: 40_500
            }
        );
        assert!(l.finish().is_empty());
    }

    #[test]
    fn short_plays_are_not_logged() {
        let mut l = Listens::default();
        l.start(id("a"), 0, 0);
        play(&mut l, "a", 0, 29);
        assert_eq!(l.end("a", Some(29_900)), None);
    }

    #[test]
    fn seeks_do_not_count() {
        let mut l = Listens::default();
        l.start(id("a"), 0, 0);
        play(&mut l, "a", 0, 10);
        // Seek forward to 3:00 (the user's seek, then the engine's report of it).
        l.seek("a", 180_000);
        play(&mut l, "a", 180, 190);
        // A small skip ahead the player announced: still not listening.
        l.seek("a", 193_000);
        play(&mut l, "a", 193, 200);
        // A jump the player did not announce is ignored too.
        play(&mut l, "a", 260, 265);
        let done = l
            .end("a", Some(265_000))
            .unwrap_or_else(|| panic!("10 + 10 + 7 + 5 s is too short"));
        assert_eq!(done.listened_ms, 32_000);
    }

    #[test]
    fn a_seek_back_replays_count() {
        let mut l = Listens::default();
        l.start(id("a"), 0, 0);
        play(&mut l, "a", 0, 20);
        l.seek("a", 0);
        play(&mut l, "a", 0, 20);
        assert_eq!(l.end("a", None).unwrap().listened_ms, 40_000);
    }

    #[test]
    fn a_play_resumed_mid_track_counts_from_there() {
        let mut l = Listens::default();
        l.start(id("a"), 95_000, 0);
        play(&mut l, "a", 95, 130);
        assert_eq!(l.end("a", None).unwrap().listened_ms, 35_000);
    }

    #[test]
    fn pauses_add_nothing() {
        let mut l = Listens::default();
        l.start(id("a"), 0, 0);
        play(&mut l, "a", 0, 20);
        // Paused: the engine reports the exact spot once, then nothing for a long time.
        l.position("a", 20_400);
        play(&mut l, "a", 21, 40);
        assert_eq!(l.end("a", None).unwrap().listened_ms, 40_000);
    }

    #[test]
    fn crossfade_overlap_and_repeat_one() {
        let mut l = Listens::default();
        l.start(id("a"), 0, 0);
        play(&mut l, "a", 0, 50);
        // The successor becomes audible while `a` fades out for 8 s; `a`'s end comes later.
        assert!(l.start(id("b"), 0, 60_000).is_empty());
        play(&mut l, "b", 0, 8);
        assert_eq!(l.end("a", Some(58_000)).unwrap().listened_ms, 58_000);
        // Repeat-one: the new play of `b` starts before the old one reports its end.
        play(&mut l, "b", 9, 40);
        l.start(id("b"), 0, 100_000);
        play(&mut l, "b", 0, 3);
        let old = l.end("b", Some(40_000)).unwrap();
        assert_eq!((old.started_at, old.listened_ms), (60_000, 40_000));
        let rest = l.end("b", None);
        assert_eq!(rest, None, "the new play has only 3 s");
    }

    #[test]
    fn plays_without_an_end_are_closed_by_newer_ones_and_on_exit() {
        let mut l = Listens::default();
        l.start(id("a"), 0, 0);
        play(&mut l, "a", 0, 35);
        l.start(id("b"), 0, 1);
        play(&mut l, "b", 0, 31);
        // `a` never reported an end: a third play pushes it out, logged.
        let closed = l.start(id("c"), 0, 2);
        assert_eq!(
            closed
                .iter()
                .map(|x| x.track_id.as_str())
                .collect::<Vec<_>>(),
            ["a"]
        );
        play(&mut l, "c", 0, 5);
        let left = l.finish();
        assert_eq!(
            left.iter().map(|x| x.track_id.as_str()).collect::<Vec<_>>(),
            ["b"],
            "c is too short"
        );
    }

    #[test]
    fn unknown_tracks_are_ignored() {
        let mut l = Listens::default();
        l.position("x", 1_000);
        l.seek("x", 1_000);
        assert_eq!(l.end("x", Some(60_000)), None);
    }
}
