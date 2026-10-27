//! The hand-off channel between the Mixtape app and the island's music module
//! (`proto = 1`): who plays, and moving the playback from one to the other.

pub mod msg;
pub mod pipe;

pub use msg::{Command, ErrorCode, FrameError, Handoff, MAX_FRAME, Msg, PROTO, Side, decode, encode};
#[cfg(windows)]
pub use pipe::pipe_name;
pub use pipe::{Link, LinkError, LinkRx, LinkTx, Peer, Server, TIMEOUT, keep_new};
