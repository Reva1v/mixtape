//! Streaming hooks: tracks without audio on disk play through a [`StreamProvider`] the
//! shell implements (URL resolution, the sparse cache file, the HTTP filler), and the
//! player tells it which remote tracks come next so it can get them ready.

use crate::queue::Queue;
use mixtape_audio::Source;
use mixtape_core::TrackId;

/// Called once with a source that can be opened, or a Russian reason it cannot.
pub type Opened = Box<dyn FnOnce(Result<Source, String>) + Send>;

/// URLs are resolved this many tracks ahead (yt-dlp takes seconds, batching helps).
pub const RESOLVE_AHEAD: usize = 5;
/// This many upcoming tracks are downloaded into the cache ahead of time.
pub const CACHE_AHEAD: usize = 2;

pub trait StreamProvider: Send + Sync {
    /// Starts streaming `id` and calls `done` once its source can be opened. Asking twice
    /// for the same track shares one download.
    fn open(&self, id: &TrackId, done: Opened);
    /// The remote tracks after the current one, nearest first (at most [`RESOLVE_AHEAD`]):
    /// resolve their URLs, cache the first [`CACHE_AHEAD`], and drop work for anything else
    /// except `current`.
    fn prefetch(&self, current: Option<&TrackId>, upcoming: &[TrackId]);
}

/// The tracks within [`RESOLVE_AHEAD`] after the current one, in play order.
pub fn window(queue: &Queue) -> Vec<TrackId> {
    queue.upcoming(RESOLVE_AHEAD).map(|i| i.track_id.clone()).collect()
}

/// The tracks of `window` that have no audio on disk, without repeats.
pub fn upcoming_remote(window: Vec<TrackId>, has_audio: impl Fn(&TrackId) -> bool) -> Vec<TrackId> {
    let mut out: Vec<TrackId> = Vec::new();
    for id in window {
        if !has_audio(&id) && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::QueueContext;

    #[test]
    fn skips_tracks_on_disk_and_stops_at_the_window() {
        let ids: Vec<TrackId> = (0..10).map(|i| TrackId(format!("t{i}"))).collect();
        let mut q = Queue::new();
        q.set(ids, 0, QueueContext::None);
        let got = upcoming_remote(window(&q), |id| id.as_str() == "t2");
        let names: Vec<&str> = got.iter().map(|t| t.as_str()).collect();
        assert_eq!(names, ["t1", "t3", "t4", "t5"]);
    }
}
