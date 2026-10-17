//! A running mix ("Включить микс"): which YouTube Music radio feeds the queue, its chips,
//! the continuation token and the tracks already handed out, so a page never repeats one.
//!
//! The session only keeps state; fetching pages is the shell's job. It asks
//! [`MixSession::begin_more`] when the queue runs low and hands the page back to
//! [`MixSession::accept_more`].

use crate::state::Chip;
use mixtape_core::TrackId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Load the next page when this many tracks or fewer are left after the current one.
pub const LOW_WATER: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixChip {
    pub id: String,
    pub title: String,
    pub playlist_id: Option<String>,
    pub params: Option<String>,
    pub selected: bool,
}

/// One page of a mix, already stored in the library as tracks.
#[derive(Debug, Clone, Default)]
pub struct MixPage {
    pub tracks: Vec<TrackId>,
    pub chips: Vec<MixChip>,
    pub continuation: Option<String>,
    pub playlist_id: Option<String>,
}

/// What to ask YouTube Music for next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoreRequest {
    pub playlist_id: String,
    pub continuation: String,
    /// Bumped by a chip switch; a page fetched for an older generation is dropped.
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixSession {
    pub seed: TrackId,
    playlist_id: Option<String>,
    chips: Vec<MixChip>,
    continuation: Option<String>,
    seen: HashSet<TrackId>,
    generation: u64,
    #[serde(skip)]
    loading: bool,
}

impl MixSession {
    /// Starts a session from the first page; returns the tracks to queue, seed first.
    pub fn start(seed: TrackId, page: MixPage) -> (Self, Vec<TrackId>) {
        let mut s = Self {
            seed: seed.clone(),
            playlist_id: page.playlist_id,
            chips: page.chips,
            continuation: page.continuation,
            seen: HashSet::new(),
            generation: 0,
            loading: false,
        };
        let mut tracks = vec![seed];
        tracks.extend(page.tracks);
        let tracks = s.fresh(tracks);
        (s, tracks)
    }

    /// Keeps the tracks not handed out yet, in order, and remembers them.
    fn fresh(&mut self, tracks: Vec<TrackId>) -> Vec<TrackId> {
        tracks.into_iter().filter(|t| self.seen.insert(t.clone())).collect()
    }

    pub fn chips(&self) -> Vec<Chip> {
        self.chips
            .iter()
            .map(|c| Chip { id: c.id.clone(), title: c.title.clone(), selected: c.selected })
            .collect()
    }

    pub fn chip(&self, id: &str) -> Option<&MixChip> {
        self.chips.iter().find(|c| c.id == id)
    }

    /// The next page to fetch when `remaining` tracks are left, unless one is already on its way.
    pub fn begin_more(&mut self, remaining: usize) -> Option<MoreRequest> {
        if self.loading || remaining > LOW_WATER {
            return None;
        }
        let req = MoreRequest {
            playlist_id: self.playlist_id.clone()?,
            continuation: self.continuation.clone()?,
            generation: self.generation,
        };
        self.loading = true;
        Some(req)
    }

    /// Takes a fetched page; returns the tracks to append (empty for a stale page).
    pub fn accept_more(&mut self, generation: u64, page: MixPage) -> Vec<TrackId> {
        if generation != self.generation {
            return Vec::new();
        }
        self.loading = false;
        self.continuation = page.continuation;
        if page.playlist_id.is_some() {
            self.playlist_id = page.playlist_id;
        }
        self.fresh(page.tracks)
    }

    /// The fetch failed; the next low-water check may try again.
    pub fn more_failed(&mut self, generation: u64) {
        if generation == self.generation {
            self.loading = false;
        }
    }

    /// Starts a chip switch: marks the chip selected and invalidates pages in flight.
    /// Returns the generation the chip's page must be accepted with.
    pub fn select_chip(&mut self, id: &str) -> Option<u64> {
        self.chip(id)?;
        for c in &mut self.chips {
            c.selected = c.id == id;
        }
        self.generation += 1;
        self.loading = false;
        Some(self.generation)
    }

    /// Takes the chip's first page. `kept` are the queue items that stay (played ones and
    /// the current track); everything else may come back. Chips stay as they were.
    pub fn accept_chip(&mut self, generation: u64, kept: &[TrackId], page: MixPage) -> Vec<TrackId> {
        if generation != self.generation {
            return Vec::new();
        }
        self.seen = kept.iter().cloned().collect();
        self.continuation = page.continuation;
        if page.playlist_id.is_some() {
            self.playlist_id = page.playlist_id;
        }
        self.fresh(page.tracks)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn can_continue(&self) -> bool {
        self.continuation.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<TrackId> {
        v.iter().map(|s| TrackId((*s).into())).collect()
    }

    fn chip(id: &str, selected: bool) -> MixChip {
        MixChip {
            id: id.into(),
            title: id.into(),
            playlist_id: Some(format!("RD{id}")),
            params: None,
            selected,
        }
    }

    fn page(tracks: &[&str], cont: Option<&str>) -> MixPage {
        MixPage {
            tracks: ids(tracks),
            chips: vec![chip("all", true), chip("familiar", false)],
            continuation: cont.map(Into::into),
            playlist_id: Some("RDAMVMs".into()),
        }
    }

    #[test]
    fn start_puts_seed_first_and_drops_its_echo() {
        let (s, tracks) = MixSession::start(TrackId("s".into()), page(&["s", "a", "b", "a"], Some("c1")));
        assert_eq!(tracks, ids(&["s", "a", "b"]));
        assert_eq!(s.chips().len(), 2);
        assert!(s.chips()[0].selected);
    }

    #[test]
    fn more_only_when_low_and_once() {
        let (mut s, _) = MixSession::start(TrackId("s".into()), page(&["a"], Some("c1")));
        assert!(s.begin_more(LOW_WATER + 1).is_none());
        let req = s.begin_more(LOW_WATER).unwrap();
        assert_eq!(req.continuation, "c1");
        assert!(s.begin_more(0).is_none(), "one fetch at a time");
        let added = s.accept_more(req.generation, page(&["a", "c", "d"], None));
        assert_eq!(added, ids(&["c", "d"]));
        assert!(s.begin_more(0).is_none(), "no continuation left");
        assert!(!s.can_continue());
    }

    #[test]
    fn failed_fetch_can_retry() {
        let (mut s, _) = MixSession::start(TrackId("s".into()), page(&[], Some("c1")));
        let req = s.begin_more(0).unwrap();
        s.more_failed(req.generation);
        assert!(s.begin_more(0).is_some());
    }

    #[test]
    fn chip_switch_drops_stale_pages_and_resets_seen() {
        let (mut s, _) = MixSession::start(TrackId("s".into()), page(&["a", "b", "c"], Some("c1")));
        let stale = s.begin_more(0).unwrap();
        let generation = s.select_chip("familiar").unwrap();
        assert!(s.chips()[1].selected && !s.chips()[0].selected);
        assert!(s.accept_more(stale.generation, page(&["x"], Some("old"))).is_empty());
        // "s" and "a" were played; "b" and "c" were dropped from the queue and may return.
        let added = s.accept_chip(generation, &ids(&["s", "a"]), page(&["a", "c", "e"], Some("c2")));
        assert_eq!(added, ids(&["c", "e"]));
        assert_eq!(s.begin_more(0).unwrap().continuation, "c2");
        assert!(s.select_chip("nope").is_none());
    }
}
