//! Playback logic above the audio engine: the queue, mixes and prefetching.

pub mod queue;

pub use queue::{Origin, Queue, QueueContext, QueueItem, Repeat};
