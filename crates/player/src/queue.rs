//! The play queue: what is playing, what comes next, repeat and shuffle.
//!
//! Items have a stable `uid`, so the same track can appear twice and the UI can address
//! a specific entry. Shuffle only reorders what is still ahead and remembers the
//! original order, so turning it off restores the album or playlist order.

use mixtape_core::{AlbumId, ArtistId, PlaylistId, TrackId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

/// How an item got into the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Origin {
    /// From the album, playlist or list the user started.
    Context,
    /// "Play next" / "Add to queue".
    User,
    /// Added by a running mix.
    Mix,
}

/// What the queue was started from; shown in the UI and used by mixes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind", content = "id")]
#[ts(export)]
pub enum QueueContext {
    #[default]
    None,
    Album(AlbumId),
    Artist(ArtistId),
    Playlist(PlaylistId),
    Library,
    Liked,
    Search,
    Mix(TrackId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueItem {
    #[ts(type = "number")]
    pub uid: u64,
    pub track_id: TrackId,
    pub origin: Origin,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Queue {
    items: Vec<QueueItem>,
    current: Option<usize>,
    repeat: Repeat,
    shuffle: bool,
    /// Order of uids before shuffle was turned on.
    original_order: Option<Vec<u64>>,
    context: QueueContext,
    next_uid: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }
    pub fn current_index(&self) -> Option<usize> {
        self.current
    }
    pub fn current(&self) -> Option<&QueueItem> {
        self.current.and_then(|i| self.items.get(i))
    }
    pub fn repeat(&self) -> Repeat {
        self.repeat
    }
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn context(&self) -> &QueueContext {
        &self.context
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn make(&mut self, track_id: TrackId, origin: Origin) -> QueueItem {
        self.next_uid += 1;
        QueueItem { uid: self.next_uid, track_id, origin }
    }

    fn index_of(&self, uid: u64) -> Option<usize> {
        self.items.iter().position(|i| i.uid == uid)
    }

    /// Replaces the queue with `tracks`, starting at `start`.
    pub fn set(&mut self, tracks: Vec<TrackId>, start: usize, context: QueueContext) {
        self.items = tracks.into_iter().map(|t| self.make(t, Origin::Context)).collect::<Vec<_>>();
        self.current = (!self.items.is_empty()).then(|| start.min(self.items.len() - 1));
        self.context = context;
        self.original_order = None;
        if self.shuffle {
            self.shuffle_upcoming();
        }
    }

    /// Inserts right after the current item ("Play next").
    pub fn play_next(&mut self, tracks: Vec<TrackId>) {
        let at = self.current.map_or(0, |c| c + 1);
        self.insert_at(at, tracks, Origin::User);
    }

    /// Inserts after the items the user already queued, before the rest of the context.
    pub fn add_to_queue(&mut self, tracks: Vec<TrackId>) {
        let mut at = self.current.map_or(0, |c| c + 1);
        while self.items.get(at).is_some_and(|i| i.origin == Origin::User) {
            at += 1;
        }
        self.insert_at(at, tracks, Origin::User);
    }

    /// Appends tracks fetched by a mix.
    pub fn append_mix(&mut self, tracks: Vec<TrackId>) {
        let at = self.items.len();
        self.insert_at(at, tracks, Origin::Mix);
    }

    /// Drops every upcoming mix item and appends `tracks` instead (a mix chip was picked).
    /// The current item and user-queued items stay.
    pub fn replace_upcoming_mix(&mut self, tracks: Vec<TrackId>) {
        let cur = self.current.unwrap_or(0);
        let mut i = 0;
        self.items.retain(|item| {
            let keep = i <= cur || item.origin != Origin::Mix;
            i += 1;
            keep
        });
        self.append_mix(tracks);
    }

    fn insert_at(&mut self, at: usize, tracks: Vec<TrackId>, origin: Origin) {
        let new: Vec<QueueItem> = tracks.into_iter().map(|t| self.make(t, origin)).collect();
        if let Some(order) = &mut self.original_order {
            order.extend(new.iter().map(|i| i.uid));
        }
        let at = at.min(self.items.len());
        self.items.splice(at..at, new);
        if self.current.is_none() && !self.items.is_empty() {
            self.current = Some(0);
        }
    }

    pub fn remove(&mut self, uid: u64) -> bool {
        let Some(idx) = self.index_of(uid) else { return false };
        self.items.remove(idx);
        if let Some(order) = &mut self.original_order {
            order.retain(|u| *u != uid);
        }
        self.current = match self.current {
            _ if self.items.is_empty() => None,
            Some(c) if idx < c => Some(c - 1),
            Some(c) => Some(c.min(self.items.len() - 1)),
            None => None,
        };
        true
    }

    /// Moves an item to `to` (index in the list after the move).
    pub fn move_item(&mut self, uid: u64, to: usize) -> bool {
        let Some(from) = self.index_of(uid) else { return false };
        let current_uid = self.current().map(|i| i.uid);
        let item = self.items.remove(from);
        let to = to.min(self.items.len());
        self.items.insert(to, item);
        self.current = current_uid.and_then(|u| self.index_of(u));
        true
    }

    /// Keeps only the current item.
    pub fn clear_upcoming(&mut self) {
        if let Some(c) = self.current {
            self.items.truncate(c + 1);
            self.items.drain(..c);
            self.current = Some(0);
        } else {
            self.items.clear();
        }
        self.original_order = None;
    }

    pub fn jump(&mut self, uid: u64) -> Option<&QueueItem> {
        self.current = Some(self.index_of(uid)?);
        self.current()
    }

    /// The item that plays after the current one. `auto` is true when the current track
    /// ended by itself: only then does "repeat one" replay it.
    pub fn peek_next(&self, auto: bool) -> Option<&QueueItem> {
        let c = self.current?;
        if auto && self.repeat == Repeat::One {
            return self.items.get(c);
        }
        match self.items.get(c + 1) {
            Some(i) => Some(i),
            None if self.repeat != Repeat::Off => self.items.first(),
            None => None,
        }
    }

    pub fn advance(&mut self, auto: bool) -> Option<&QueueItem> {
        let uid = self.peek_next(auto)?.uid;
        self.jump(uid)
    }

    /// Previous item; wraps to the end only with repeat on.
    pub fn back(&mut self) -> Option<&QueueItem> {
        let c = self.current?;
        let idx = match c.checked_sub(1) {
            Some(i) => i,
            None if self.repeat != Repeat::Off => self.items.len().checked_sub(1)?,
            None => return None,
        };
        self.current = Some(idx);
        self.current()
    }

    /// Up to `n` items after the current one, for prefetching.
    pub fn upcoming(&self, n: usize) -> impl Iterator<Item = &QueueItem> {
        let start = self.current.map_or(0, |c| c + 1);
        self.items.iter().skip(start).take(n)
    }

    pub fn remaining(&self) -> usize {
        self.current.map_or(self.items.len(), |c| self.items.len() - c - 1)
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            self.original_order = Some(self.items.iter().map(|i| i.uid).collect());
            self.shuffle_upcoming();
        } else if let Some(order) = self.original_order.take() {
            self.restore_order(&order);
        }
    }

    fn shuffle_upcoming(&mut self) {
        let start = self.current.map_or(0, |c| c + 1);
        if start < self.items.len() {
            fastrand::shuffle(&mut self.items[start..]);
        }
    }

    /// Puts items back in their pre-shuffle order; the current item keeps playing.
    fn restore_order(&mut self, order: &[u64]) {
        let rank = |uid: u64| order.iter().position(|u| *u == uid).unwrap_or(usize::MAX);
        let current_uid = self.current().map(|i| i.uid);
        self.items.sort_by_key(|i| rank(i.uid));
        self.current = current_uid.and_then(|u| self.index_of(u));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<TrackId> {
        names.iter().map(|n| TrackId::from(*n)).collect()
    }

    fn order(q: &Queue) -> Vec<&str> {
        q.items().iter().map(|i| i.track_id.as_str()).collect()
    }

    fn cur(q: &Queue) -> &str {
        q.current().unwrap().track_id.as_str()
    }

    #[test]
    fn plays_through_and_repeats() {
        let mut q = Queue::new();
        q.set(ids(&["a", "b", "c"]), 1, QueueContext::Library);
        assert_eq!(cur(&q), "b");
        assert_eq!(q.advance(true).unwrap().track_id.as_str(), "c");
        assert!(q.advance(true).is_none());
        q.set_repeat(Repeat::All);
        assert_eq!(q.advance(true).unwrap().track_id.as_str(), "a");
        q.set_repeat(Repeat::One);
        assert_eq!(q.advance(true).unwrap().track_id.as_str(), "a");
        // A manual "next" skips even with repeat one.
        assert_eq!(q.advance(false).unwrap().track_id.as_str(), "b");
        assert_eq!(q.back().unwrap().track_id.as_str(), "a");
        assert_eq!(q.back().unwrap().track_id.as_str(), "c"); // wraps with repeat on
    }

    #[test]
    fn play_next_and_add_to_queue_order() {
        let mut q = Queue::new();
        q.set(ids(&["a", "b", "c"]), 0, QueueContext::Library);
        q.add_to_queue(ids(&["x"]));
        q.add_to_queue(ids(&["y"]));
        q.play_next(ids(&["n"]));
        assert_eq!(order(&q), ["a", "n", "x", "y", "b", "c"]);
        assert_eq!(q.items()[1].origin, Origin::User);
    }

    #[test]
    fn remove_and_move_keep_current() {
        let mut q = Queue::new();
        q.set(ids(&["a", "b", "c", "d"]), 2, QueueContext::Library);
        let a = q.items()[0].uid;
        assert!(q.remove(a));
        assert_eq!(cur(&q), "c");
        let d = q.items()[2].uid;
        assert!(q.move_item(d, 0));
        assert_eq!(order(&q), ["d", "b", "c"]);
        assert_eq!(cur(&q), "c");
        let c = q.current().unwrap().uid;
        q.remove(c);
        assert_eq!(cur(&q), "b"); // index clamped to the last item
    }

    #[test]
    fn shuffle_only_touches_upcoming_and_restores() {
        let names: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
        let tracks: Vec<TrackId> = names.iter().map(|n| TrackId::from(n.as_str())).collect();
        let mut q = Queue::new();
        q.set(tracks.clone(), 5, QueueContext::Library);
        q.set_shuffle(true);
        assert_eq!(&order(&q)[..6], &names[..6].iter().map(String::as_str).collect::<Vec<_>>()[..]);
        assert_eq!(cur(&q), "t5");
        assert_ne!(order(&q), names.iter().map(String::as_str).collect::<Vec<_>>());
        q.advance(false);
        let playing = cur(&q).to_owned();
        q.set_shuffle(false);
        assert_eq!(order(&q), names.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(cur(&q), playing);
    }

    #[test]
    fn mix_items_are_replaced_by_chip() {
        let mut q = Queue::new();
        q.set(ids(&["seed"]), 0, QueueContext::Mix(TrackId::from("seed")));
        q.append_mix(ids(&["m1", "m2", "m3"]));
        q.advance(false);
        q.add_to_queue(ids(&["u"]));
        q.replace_upcoming_mix(ids(&["c1", "c2"]));
        assert_eq!(order(&q), ["seed", "m1", "u", "c1", "c2"]);
        assert_eq!(cur(&q), "m1");
        assert_eq!(q.remaining(), 3);
    }

    #[test]
    fn clear_keeps_current() {
        let mut q = Queue::new();
        q.set(ids(&["a", "b", "c"]), 1, QueueContext::Library);
        q.clear_upcoming();
        assert_eq!(order(&q), ["b"]);
        assert_eq!(cur(&q), "b");
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut q = Queue::new();
        q.set(ids(&["a", "b"]), 1, QueueContext::Album(AlbumId::from("MPREx")));
        q.set_shuffle(true);
        let json = serde_json::to_string(&q).unwrap();
        let back: Queue = serde_json::from_str(&json).unwrap();
        assert_eq!(back.items(), q.items());
        assert_eq!(back.current_index(), Some(1));
        assert!(back.shuffle());
    }
}
