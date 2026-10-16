//! Maps the ring's frame counter to tracks: the engine records where each track starts in the
//! stream of frames it pushed, the device reports how many it consumed, and this turns the
//! difference into positions and start/end transitions of what is actually audible.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Transition {
    /// `seq` identifies the pipeline that produced the track.
    Started {
        seq: u64,
        id: String,
        duration_ms: u64,
    },
    Ended {
        id: String,
        played_ms: u64,
        completed: bool,
    },
}

#[derive(Debug)]
enum Mark {
    Track {
        seq: u64,
        id: String,
        start_ms: u64,
        duration_ms: u64,
        started: bool,
    },
    End,
}

#[derive(Debug)]
pub(crate) struct Timeline {
    rate: u32,
    /// `(ring frame index, mark)` in push order.
    marks: VecDeque<(u64, Mark)>,
}

impl Timeline {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            marks: VecDeque::new(),
        }
    }

    pub fn clear(&mut self, rate: u32) {
        self.rate = rate;
        self.marks.clear();
    }

    /// Track `id` begins at ring frame `at`, `start_ms` into the track. `started` skips the
    /// Started transition (re-priming a track that was already audible).
    pub fn push_track(
        &mut self,
        at: u64,
        seq: u64,
        id: &str,
        start_ms: u64,
        duration_ms: u64,
        started: bool,
    ) {
        if matches!(self.marks.back(), Some((end_at, Mark::End)) if *end_at >= at) {
            self.marks.pop_back();
        }
        let mark = Mark::Track {
            seq,
            id: id.to_string(),
            start_ms,
            duration_ms,
            started,
        };
        self.marks.push_back((at, mark));
    }

    /// Nothing follows the last pushed frame, which is ring frame `at - 1`.
    pub fn push_end(&mut self, at: u64) {
        if !matches!(self.marks.back(), Some((_, Mark::End))) {
            self.marks.push_back((at, Mark::End));
        }
    }

    pub fn ended(&self) -> bool {
        matches!(self.marks.front(), Some((_, Mark::End)))
    }

    /// Whether the audible track has already reported Started.
    pub fn started(&self) -> bool {
        matches!(
            self.marks.front(),
            Some((_, Mark::Track { started: true, .. }))
        )
    }

    /// Emits transitions for everything the device has consumed up to `consumed` frames.
    pub fn advance(&mut self, consumed: u64, out: &mut Vec<Transition>) {
        loop {
            if let Some((
                at,
                Mark::Track {
                    seq,
                    id,
                    duration_ms,
                    started,
                    ..
                },
            )) = self.marks.front_mut()
                && !*started
                && consumed > *at
            {
                *started = true;
                let (seq, id, duration_ms) = (*seq, id.clone(), *duration_ms);
                out.push(Transition::Started {
                    seq,
                    id,
                    duration_ms,
                });
            }
            let next_reached = matches!(self.marks.get(1), Some((at, mark))
                if consumed > *at || (matches!(mark, Mark::End) && consumed >= *at));
            if !next_reached {
                return;
            }
            let next_at = self.marks[1].0;
            if let Some((
                _,
                Mark::Track {
                    id, started: true, ..
                },
            )) = self.marks.front()
            {
                let played_ms = self.ms_at(0, next_at);
                out.push(Transition::Ended {
                    id: id.clone(),
                    played_ms,
                    completed: true,
                });
            }
            self.marks.pop_front();
        }
    }

    /// The audible track and position at `consumed`.
    pub fn position(&self, consumed: u64) -> Option<(&str, u64)> {
        match self.marks.front() {
            Some((at, Mark::Track { id, .. })) => Some((id, self.ms_at(0, consumed.max(*at)))),
            _ => None,
        }
    }

    /// Ends the audible track early (load/stop); the caller clears or rebuilds the rest.
    pub fn interrupt(&mut self, consumed: u64) -> Option<Transition> {
        let (at, mark) = self.marks.front()?;
        let Mark::Track {
            id, started: true, ..
        } = mark
        else {
            return None;
        };
        let played_ms = self.ms_at(0, consumed.max(*at));
        Some(Transition::Ended {
            id: id.clone(),
            played_ms,
            completed: false,
        })
    }

    fn ms_at(&self, idx: usize, frame: u64) -> u64 {
        match &self.marks[idx] {
            (at, Mark::Track { start_ms, .. }) => {
                start_ms + frame.saturating_sub(*at) * 1000 / self.rate as u64
            }
            (_, Mark::End) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut Timeline, consumed: u64) -> Vec<Transition> {
        let mut out = Vec::new();
        t.advance(consumed, &mut out);
        out
    }

    fn started(seq: u64, id: &str, duration_ms: u64) -> Transition {
        Transition::Started {
            seq,
            id: id.into(),
            duration_ms,
        }
    }

    fn ended(id: &str, played_ms: u64, completed: bool) -> Transition {
        Transition::Ended {
            id: id.into(),
            played_ms,
            completed,
        }
    }

    #[test]
    fn positions_follow_consumed_frames() {
        let mut t = Timeline::new(48_000);
        t.push_track(1000, 1, "a", 30_000, 180_000, false);
        assert_eq!(run(&mut t, 1000), vec![]);
        assert_eq!(t.position(1000), Some(("a", 30_000)));
        assert_eq!(run(&mut t, 1001), vec![started(1, "a", 180_000)]);
        assert_eq!(t.position(1000 + 48_000), Some(("a", 31_000)));
        assert_eq!(t.position(1000 + 24_000), Some(("a", 30_500)));
    }

    #[test]
    fn gapless_handover_and_end() {
        let mut t = Timeline::new(48_000);
        t.push_track(0, 1, "a", 0, 2000, false);
        t.push_track(96_000, 2, "b", 0, 1000, false);
        t.push_end(144_000);
        assert_eq!(run(&mut t, 10), vec![started(1, "a", 2000)]);
        assert_eq!(run(&mut t, 96_000), vec![]);
        assert_eq!(t.position(96_000), Some(("a", 2000)));
        assert_eq!(
            run(&mut t, 96_480),
            vec![ended("a", 2000, true), started(2, "b", 1000)]
        );
        assert_eq!(t.position(96_480), Some(("b", 10)));
        assert!(!t.ended());
        assert_eq!(run(&mut t, 144_000), vec![ended("b", 1000, true)]);
        assert!(t.ended());
        assert_eq!(t.position(150_000), None);
    }

    #[test]
    fn late_next_replaces_pending_end() {
        let mut t = Timeline::new(1000);
        t.push_track(0, 1, "a", 0, 100, false);
        t.push_end(100);
        t.push_track(100, 2, "b", 0, 50, false);
        run(&mut t, 1);
        assert_eq!(
            run(&mut t, 101),
            vec![ended("a", 100, true), started(2, "b", 50)]
        );
    }

    #[test]
    fn interrupt_and_reprime() {
        let mut t = Timeline::new(1000);
        t.push_track(0, 1, "a", 0, 100, false);
        assert_eq!(t.interrupt(0), None, "never audible");
        run(&mut t, 5);
        assert_eq!(t.interrupt(40), Some(ended("a", 40, false)));
        t.clear(1000);
        t.push_track(0, 1, "a", 40, 100, true);
        assert_eq!(run(&mut t, 10), vec![]);
        assert_eq!(t.position(10), Some(("a", 50)));
    }
}
