//! Playback logic above the audio engine: the queue, mixes and prefetching.

pub mod player;
pub mod queue;
pub mod state;

pub use player::{EventSink, Player, PlayerEvent};
pub use queue::{Origin, Queue, QueueContext, QueueItem, Repeat};
pub use state::{Chip, PlaySource, PlaybackState, QueueEntry, QueueView, Status};
