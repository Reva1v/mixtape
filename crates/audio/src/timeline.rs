//! Maps the ring's frame counter to tracks: the engine records where each track starts in the
//! stream of frames it pushed, the device reports how many it consumed, and this turns the
//! difference into positions and start/end transitions of what is actually audible.
//!
//! A crossfaded track starts while its predecessor is still sounding: the predecessor stops
//! being "the audible track" at the start of the overlap (positions refer to the newcomer) but
//! only reports Ended once its last frame has played.

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

/// How a track takes over from its predecessor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Handover {
    /// The predecessor ends where this track starts.
    Cut,
    /// The predecessor fades out under this track for `frames` more; `completed` is what its
    /// Ended reports.
    Overlap { frames: u64, completed: bool },
}

#[derive(Debug)]
enum Mark {
    Track {
        seq: u64,
        id: String,
        start_ms: u64,
        duration_ms: u64,
        started: bool,
        handover: Handover,
    },
    End,
}

/// A track still fading out under the audible one.
#[derive(Debug)]
struct Outgoing {
    id: String,
    /// Its mark's ring frame and track position there.
    at: u64,
    start_ms: u64,
    /// Ring frame after its last one.
    end: u64,
    completed: bool,
}

#[derive(Debug)]
pub(crate) struct Timeline {
    rate: u32,
    /// `(ring frame index, mark)` in push order.
    marks: VecDeque<(u64, Mark)>,
    outgoing: Option<Outgoing>,
}

impl Timeline {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            marks: VecDeque::new(),
            outgoing: None,
        }
    }

    pub fn clear(&mut self, rate: u32) {
        self.rate = rate;
        self.marks.clear();
        self.outgoing = None;
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
            handover: Handover::Cut,
        };
        self.marks.push_back((at, mark));
    }

    /// Sets how the last pushed track takes over from the one before it.
    pub fn set_handover(&mut self, h: Handover) {
        if let Some((_, Mark::Track { handover, .. })) = self.marks.back_mut() {
            *handover = h;
        }
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
            if self.outgoing.as_ref().is_some_and(|o| consumed >= o.end) {
                out.extend(self.take_outgoing(consumed));
            }
            let next_reached = matches!(self.marks.get(1), Some((at, mark))
                if consumed > *at || (matches!(mark, Mark::End) && consumed >= *at));
            if !next_reached {
                return;
            }
            let next_at = self.marks[1].0;
            let overlap = match self.marks[1].1 {
                Mark::Track {
                    handover: Handover::Overlap { frames, completed },
                    ..
                } => Some((frames, completed)),
                _ => None,
            };
            if let Some((
                at,
                Mark::Track {
                    id,
                    start_ms,
                    started: true,
                    ..
                },
            )) = self.marks.front()
            {
                let (at, id, start_ms) = (*at, id.clone(), *start_ms);
                // A fade still running is cut by the next handover.
                out.extend(self.take_outgoing(next_at));
                match overlap {
                    Some((frames, completed)) => {
                        self.outgoing = Some(Outgoing {
                            id,
                            at,
                            start_ms,
                            end: next_at + frames,
                            completed,
                        });
                    }
                    None => out.push(Transition::Ended {
                        id,
                        played_ms: self.ms(at, start_ms, next_at),
                        completed: true,
                    }),
                }
            }
            self.marks.pop_front();
        }
    }

    /// The audible track and position at `consumed`.
    pub fn position(&self, consumed: u64) -> Option<(&str, u64)> {
        match self.marks.front() {
            Some((at, Mark::Track { id, start_ms, .. })) => {
                Some((id, self.ms(*at, *start_ms, consumed.max(*at))))
            }
            _ => None,
        }
    }

    /// Ends the track fading out under the audible one, if any, as of `consumed`.
    pub fn take_outgoing(&mut self, consumed: u64) -> Option<Transition> {
        let o = self.outgoing.take()?;
        Some(Transition::Ended {
            played_ms: self.ms(o.at, o.start_ms, consumed.min(o.end)),
            id: o.id,
            completed: o.completed,
        })
    }

    /// Ends what is audible early (load/stop): a track still fading out, then the audible
    /// one. The caller clears or rebuilds the rest.
    pub fn interrupt(&mut self, consumed: u64, out: &mut Vec<Transition>) {
        out.extend(self.take_outgoing(consumed));
        if let Some((
            at,
            Mark::Track {
                id,
                start_ms,
                started: true,
                ..
            },
        )) = self.marks.front()
        {
            out.push(Transition::Ended {
                id: id.clone(),
                played_ms: self.ms(*at, *start_ms, consumed.max(*at)),
                completed: false,
            });
        }
    }

    /// Track position at ring frame `frame` for a mark at `at` that starts `start_ms` in.
    fn ms(&self, at: u64, start_ms: u64, frame: u64) -> u64 {
        start_ms + frame.saturating_sub(at) * 1000 / self.rate as u64
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

    fn interrupt(t: &mut Timeline, consumed: u64) -> Vec<Transition> {
        let mut out = Vec::new();
        t.interrupt(consumed, &mut out);
        out
    }

    #[test]
    fn interrupt_and_reprime() {
        let mut t = Timeline::new(1000);
        t.push_track(0, 1, "a", 0, 100, false);
        assert_eq!(interrupt(&mut t, 0), vec![], "never audible");
        run(&mut t, 5);
        assert_eq!(interrupt(&mut t, 40), vec![ended("a", 40, false)]);
        t.clear(1000);
        t.push_track(0, 1, "a", 40, 100, true);
        assert_eq!(run(&mut t, 10), vec![]);
        assert_eq!(t.position(10), Some(("a", 50)));
    }

    #[test]
    fn crossfade_starts_at_overlap_and_ends_after_it() {
        let overlap = Handover::Overlap {
            frames: 20,
            completed: true,
        };
        let mut t = Timeline::new(1000);
        t.push_track(0, 1, "a", 0, 100, false);
        t.push_track(80, 2, "b", 0, 50, false);
        t.set_handover(overlap);
        t.push_end(130);
        assert_eq!(run(&mut t, 1), vec![started(1, "a", 100)]);
        assert_eq!(run(&mut t, 80), vec![]);
        assert_eq!(run(&mut t, 81), vec![started(2, "b", 50)]);
        assert_eq!(
            t.position(90),
            Some(("b", 10)),
            "positions follow the newcomer"
        );
        assert!(t.started());
        assert_eq!(run(&mut t, 99), vec![]);
        assert_eq!(run(&mut t, 100), vec![ended("a", 100, true)]);
        assert_eq!(run(&mut t, 130), vec![ended("b", 50, true)]);
        assert!(t.ended());

        // Both transitions in one step come out in order.
        let mut t = Timeline::new(1000);
        t.push_track(0, 1, "a", 0, 100, true);
        t.push_track(80, 2, "b", 0, 50, false);
        t.set_handover(overlap);
        assert_eq!(
            run(&mut t, 110),
            vec![started(2, "b", 50), ended("a", 100, true)]
        );
    }

    #[test]
    fn manual_crossfade_interrupted_mid_overlap() {
        let manual = Handover::Overlap {
            frames: 20,
            completed: false,
        };
        // Re-primed "a" at 30 ms with "b" fading in at the same frame.
        let mut t = Timeline::new(1000);
        t.push_track(0, 1, "a", 30, 100, true);
        t.push_track(0, 2, "b", 0, 50, false);
        t.set_handover(manual);
        assert_eq!(run(&mut t, 1), vec![started(2, "b", 50)]);
        assert_eq!(
            interrupt(&mut t, 11),
            vec![ended("a", 41, false), ended("b", 11, false)]
        );

        // A seek drops the fading track on its own.
        let mut t = Timeline::new(1000);
        t.push_track(0, 1, "a", 0, 100, true);
        t.push_track(0, 2, "b", 0, 50, false);
        t.set_handover(manual);
        run(&mut t, 5);
        assert_eq!(t.take_outgoing(5), Some(ended("a", 5, false)));
        assert_eq!(run(&mut t, 30), vec![]);
    }
}
