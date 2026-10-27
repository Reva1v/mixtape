//! Messages of the hand-off channel and their framing: one line of UTF-8 JSON ending in
//! `\n`, at most [`MAX_FRAME`] bytes (specs/010-music-module/contracts/handoff.md in the island).

use mixtape_library::views::ViewKind;
use mixtape_player::{MixSession, Queue, Snapshot};
use serde::{Deserialize, Serialize};

/// The protocol version. Optional new fields keep it (`#[serde(default)]`, unknown fields
/// are ignored); a change of meaning bumps it.
pub const PROTO: u16 = 1;

/// Longest frame, the `\n` included.
pub const MAX_FRAME: usize = 1 << 20;

/// Which app a side is: the standalone Mixtape app or the island's music module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    #[serde(rename = "mixtape")]
    App,
    #[serde(rename = "island")]
    Island,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::App => Side::Island,
            Side::Island => Side::App,
        }
    }

    /// The pipe-name suffix of this side's server.
    pub fn suffix(self) -> &'static str {
        match self {
            Side::App => "app",
            Side::Island => "island",
        }
    }
}

/// What the owner hands over: the player's [`Snapshot`] and whether it was playing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub queue: Queue,
    pub position_ms: u64,
    pub playing: bool,
    pub volume: f32,
    #[serde(default)]
    pub mix: Option<MixSession>,
}

impl Handoff {
    pub fn from_snapshot(s: Snapshot, playing: bool) -> Self {
        Self { queue: s.queue, position_ms: s.position_ms, playing, volume: s.volume, mix: s.mix }
    }

    /// The snapshot to restore and whether it was playing.
    pub fn into_snapshot(self) -> (Snapshot, bool) {
        (Snapshot { queue: self.queue, position_ms: self.position_ms, volume: self.volume, mix: self.mix }, self.playing)
    }
}

/// A button of the island that SMTC has no verb for, run by the standalone app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Command {
    /// An empty `id` is the app's current track (the island knows it only through SMTC);
    /// the same for `unlike`.
    Like { id: String },
    Unlike { id: String },
    #[serde(rename_all = "camelCase")]
    StartMix {
        #[serde(default)]
        seed: Option<String>,
    },
    Enqueue {
        ids: Vec<String>,
        #[serde(default)]
        next: bool,
    },
    Jump { uid: u64 },
    Remove { uid: u64 },
    Move { uid: u64, to: usize },
    ShowWindow {
        #[serde(default)]
        screen: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Incompatible,
    Busy,
    TooLarge,
    BadFrame,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Msg {
    Hello {
        proto: u16,
        app: Side,
        pid: u32,
        #[serde(default)]
        version: String,
    },
    /// The app started: "give me the playback".
    Claim,
    /// "Play from this"; the answer is `ack`.
    Handoff { state: Handoff },
    /// The answer to `claim` when the island has nothing to give.
    Idle,
    Ack {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The app quits (not to the tray).
    Bye {
        #[serde(default)]
        state: Option<Handoff>,
    },
    Cmd { cmd: Command },
    /// The database changed: drop cached views of these kinds.
    Changed { kinds: Vec<ViewKind> },
    /// Closed right after.
    Error {
        code: ErrorCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proto: Option<u16>,
    },
}

impl Msg {
    /// This process's `hello`.
    pub fn hello(me: Side) -> Msg {
        Msg::Hello { proto: PROTO, app: me, pid: std::process::id(), version: env!("CARGO_PKG_VERSION").into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("frame over {MAX_FRAME} bytes")]
    TooLarge,
    #[error("bad frame: {0}")]
    Bad(String),
}

impl FrameError {
    pub fn code(&self) -> ErrorCode {
        match self {
            FrameError::TooLarge => ErrorCode::TooLarge,
            FrameError::Bad(_) => ErrorCode::BadFrame,
        }
    }
}

/// One frame: the JSON line and its `\n`.
pub fn encode(msg: &Msg) -> Result<Vec<u8>, FrameError> {
    let mut out = serde_json::to_vec(msg).map_err(|e| FrameError::Bad(e.to_string()))?;
    out.push(b'\n');
    if out.len() > MAX_FRAME {
        return Err(FrameError::TooLarge);
    }
    Ok(out)
}

/// Reads one frame. `buf` holds what was read, the `\n` included when it was found: a line
/// longer than [`MAX_FRAME`] is `TooLarge`, a line cut off by the end of the stream `Bad`.
pub fn decode(line: &[u8]) -> Result<Msg, FrameError> {
    if line.len() > MAX_FRAME {
        return Err(FrameError::TooLarge);
    }
    let Some(body) = line.strip_suffix(b"\n") else {
        return Err(FrameError::Bad("the line has no end".into()));
    };
    let body = body.strip_suffix(b"\r").unwrap_or(body);
    serde_json::from_slice(body).map_err(|e| FrameError::Bad(e.to_string()))
}

/// The `proto` of a `hello` line, read before the rest, so a peer of another version is
/// told apart from a broken frame even if its `hello` gained or lost fields.
pub fn hello_proto(line: &[u8]) -> Option<u16> {
    let v: serde_json::Value = serde_json::from_slice(line.strip_suffix(b"\n").unwrap_or(line)).ok()?;
    if v.get("t")?.as_str()? != "hello" {
        return None;
    }
    v.get("proto")?.as_u64().and_then(|p| u16::try_from(p).ok())
}
