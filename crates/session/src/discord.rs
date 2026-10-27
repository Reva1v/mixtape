//! Discord Rich Presence: "Listening to …" with the playing track, over the Discord
//! client's local IPC pipe (`\\.\pipe\discord-ipc-0` … `9`).
//!
//! Wire format: each frame is a little-endian `u32` opcode and `u32` length, then that
//! many bytes of JSON. The client sends a handshake (`{"v":1,"client_id":…}`), Discord
//! answers with a `READY` dispatch, then `SET_ACTIVITY` commands set or clear the activity.
//!
//! Costs nothing when unused: the worker thread starts on the first track played with the
//! setting on and sleeps on its channel. When Discord is not running it tries to connect
//! at most once a minute, and only while something plays. Updates go out on track
//! changes, pause/resume and seeks (not on the player's periodic position samples), at
//! most one per [`MIN_GAP`] so fast skipping stays within Discord's rate limit.

use crate::state::Session;
use mixtape_core::{Track, tr};
use mixtape_player::{PlaybackState, PlayerEvent, Status};
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::Manager;

/// The Discord application ("Mixtape") the presence belongs to; absent = feature hidden.
pub const APP_ID: Option<&str> = match option_env!("MIXTAPE_DISCORD_APP_ID") {
    Some(id) if !id.is_empty() => Some(id),
    _ => None,
};

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;
/// Larger frames are not from Discord.
const MAX_FRAME: usize = 1 << 20;
/// Connection attempts while Discord is not running.
const RETRY_EVERY: Duration = Duration::from_secs(60);
/// Updates closer together than this are merged (Discord allows 5 per 20 s).
const MIN_GAP: Duration = Duration::from_secs(4);
/// A seek shifts the start by more than this before the activity is updated.
const SEEK_SLACK_MS: i64 = 2_000;
/// Art asset uploaded to the Discord application, for tracks without a web cover.
const FALLBACK_IMAGE: &str = "mixtape";
/// A pause keeps the status this long (as «Paused»), then it goes.
const PAUSE_KEEP_MS: i64 = 30_000;

// ---------- wire format ----------

pub fn encode(op: u32, payload: &Value) -> Vec<u8> {
    let body = payload.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn read_frame(r: &mut impl Read) -> io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let op = u32::from_le_bytes(head[..4].try_into().expect("4 bytes"));
    let len = u32::from_le_bytes(head[4..].try_into().expect("4 bytes")) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let v =
        serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((op, v))
}

/// What the presence shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub track_id: String,
    pub title: String,
    pub artists: String,
    pub album: Option<String>,
    /// Unix ms when the track would have started, and when it ends.
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    /// An https cover, or `None` for the app's own picture.
    pub image: Option<String>,
    /// «Open in YouTube Music».
    pub link: Option<String>,
    /// Paused: no time bar, «Paused» after the artist.
    pub paused: bool,
}

/// Discord wants 2–128 characters in text fields.
fn field(s: &str) -> String {
    let mut s: String = s.trim().chars().take(128).collect();
    while s.chars().count() < 2 {
        s.push('\u{200b}');
    }
    s
}

/// The `SET_ACTIVITY` command; `None` clears the presence.
pub fn set_activity(pid: u32, nonce: u64, a: Option<&Activity>) -> Value {
    let mut args = json!({ "pid": pid });
    if let Some(a) = a {
        let mut act = json!({
            // Listening.
            "type": 2,
            "details": field(&a.title),
            "state": field(&if a.paused {
                format!("{} · {}", a.artists, tr("Paused", "На паузі", "На паузе"))
            } else {
                a.artists.clone()
            }),
            "assets": { "large_image": a.image.as_deref().unwrap_or(FALLBACK_IMAGE) },
        });
        if !a.paused {
            act["timestamps"] = json!({ "start": a.start_ms });
            if let Some(end) = a.end_ms {
                act["timestamps"]["end"] = json!(end);
            }
        }
        if let Some(album) = a.album.as_deref().filter(|s| !s.trim().is_empty()) {
            act["assets"]["large_text"] = json!(field(album));
        }
        if let Some(url) = &a.link {
            let label = tr(
                "Open in YouTube Music",
                "Відкрити в YouTube Music",
                "Открыть в YouTube Music",
            );
            act["buttons"] = json!([{ "label": label, "url": url }]);
        }
        args["activity"] = act;
    }
    json!({ "cmd": "SET_ACTIVITY", "args": args, "nonce": nonce.to_string() })
}

/// A connected IPC session over any byte stream (the pipe, or a fake in tests).
pub struct Client<T: Read + Write> {
    io: T,
    nonce: u64,
}

impl<T: Read + Write> Client<T> {
    /// Sends the handshake and waits for `READY`.
    pub fn connect(mut io: T, client_id: &str) -> io::Result<Self> {
        io.write_all(&encode(
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": client_id }),
        ))?;
        io.flush()?;
        let mut c = Self { io, nonce: 0 };
        let v = c.reply()?;
        if v.get("evt").and_then(Value::as_str) != Some("READY") {
            return Err(io::Error::other(format!("unexpected handshake reply: {v}")));
        }
        Ok(c)
    }

    /// The next command reply or event, answering pings on the way.
    fn reply(&mut self) -> io::Result<Value> {
        loop {
            match read_frame(&mut self.io)? {
                (OP_FRAME, v) => return Ok(v),
                (OP_PING, v) => {
                    self.io.write_all(&encode(OP_PONG, &v))?;
                    self.io.flush()?;
                }
                (OP_CLOSE, v) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        v.to_string(),
                    ));
                }
                _ => {}
            }
        }
    }

    pub fn set(&mut self, a: Option<&Activity>) -> io::Result<()> {
        self.nonce += 1;
        self.io.write_all(&encode(
            OP_FRAME,
            &set_activity(std::process::id(), self.nonce, a),
        ))?;
        self.io.flush()?;
        let v = self.reply()?;
        if v.get("evt").and_then(Value::as_str) == Some("ERROR") {
            // Discord refused the activity; the connection is still fine.
            tracing::warn!(reply = %v, "Discord rejected the activity");
        }
        Ok(())
    }
}

#[cfg(windows)]
fn open_pipe() -> Option<std::fs::File> {
    (0..10).find_map(|i| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!(r"\\.\pipe\discord-ipc-{i}"))
            .ok()
    })
}

#[cfg(not(windows))]
fn open_pipe() -> Option<std::fs::File> {
    None
}

// ---------- worker ----------

enum Msg {
    Set(Option<Activity>),
    /// The setting was turned off: clear and hang up.
    Close,
}

struct Worker {
    client_id: &'static str,
    conn: Option<Client<std::fs::File>>,
    want: Option<Activity>,
    /// What Discord shows now (as far as this connection knows).
    shown: Option<Activity>,
    last_try: Option<Instant>,
    last_send: Option<Instant>,
}

impl Worker {
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.deadline() {
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                },
            };
            if let Some(m) = msg {
                self.apply(m);
                // Only the latest state matters.
                while let Ok(m) = rx.try_recv() {
                    self.apply(m);
                }
            }
            self.sync();
        }
    }

    fn apply(&mut self, m: Msg) {
        match m {
            Msg::Set(a) => self.want = a,
            Msg::Close => {
                self.want = None;
                if let Some(mut c) = self.conn.take() {
                    let _ = c.set(None);
                }
                self.shown = None;
            }
        }
    }

    /// When the loop has to wake up without a message: a pending update held back by
    /// [`MIN_GAP`], or the next connection attempt while music plays.
    fn deadline(&self) -> Option<Instant> {
        if self.conn.is_some() {
            return (self.want != self.shown)
                .then(|| self.last_send.map_or_else(Instant::now, |t| t + MIN_GAP));
        }
        self.want.as_ref()?;
        Some(self.last_try.map_or_else(Instant::now, |t| t + RETRY_EVERY))
    }

    fn sync(&mut self) {
        if self.conn.is_none() {
            if self.want.is_none() || self.last_try.is_some_and(|t| t.elapsed() < RETRY_EVERY) {
                return;
            }
            self.last_try = Some(Instant::now());
            let Some(pipe) = open_pipe() else { return };
            match Client::connect(pipe, self.client_id) {
                Ok(c) => {
                    tracing::info!("connected to Discord");
                    self.conn = Some(c);
                    self.shown = None;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "Discord handshake failed");
                    return;
                }
            }
        }
        if self.want == self.shown || self.last_send.is_some_and(|t| t.elapsed() < MIN_GAP) {
            return;
        }
        let Some(c) = self.conn.as_mut() else { return };
        self.last_send = Some(Instant::now());
        match c.set(self.want.as_ref()) {
            Ok(()) => self.shown = self.want.clone(),
            Err(e) => {
                tracing::info!(error = %e, "Discord connection lost");
                self.conn = None;
                self.shown = None;
                // Discord may have just restarted: the next attempt may come at once.
                self.last_try = None;
            }
        }
    }
}

// ---------- app glue ----------

/// The playing track as the presence needs it.
#[derive(Clone)]
struct Current {
    track: Track,
}

#[derive(Default)]
struct Inner {
    current: Option<Current>,
    last: Option<PlaybackState>,
    /// The last activity handed to the worker.
    sent: Option<Activity>,
}

pub struct Presence {
    enabled: AtomicBool,
    inner: Mutex<Inner>,
    tx: OnceLock<Sender<Msg>>,
    /// Square album art per track id from YouTube Music's `player` answer; `None` while
    /// asked or when it has none (the video thumbnail is used then).
    art: Mutex<std::collections::HashMap<String, Option<String>>>,
}

impl Presence {
    fn send(&self, m: Msg) {
        let Some(id) = APP_ID else { return };
        let tx = self.tx.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let w = Worker {
                client_id: id,
                conn: None,
                want: None,
                shown: None,
                last_try: None,
                last_send: None,
            };
            if let Err(e) = std::thread::Builder::new()
                .name("discord-rpc".into())
                .spawn(move || w.run(rx))
            {
                tracing::warn!(error = %e, "Discord presence not started");
            }
            tx
        });
        let _ = tx.send(m);
    }
}

pub fn install(app: &tauri::AppHandle, enabled: bool) {
    app.manage(Presence {
        enabled: AtomicBool::new(enabled),
        inner: Mutex::default(),
        tx: OnceLock::new(),
        art: Mutex::default(),
    });
}

fn is_youtube(id: &str) -> bool {
    id.len() == 11 && !id.starts_with("local")
}

/// The activity for a track and a player state, or `None` when nothing plays.
fn desired(track: &Track, s: &PlaybackState, art: Option<&str>, now_ms: i64) -> Option<Activity> {
    let paused = match s.status {
        Status::Playing | Status::Buffering | Status::Loading => false,
        Status::Paused if now_ms - (s.timestamp_ms as i64) < PAUSE_KEEP_MS => true,
        _ => return None,
    };
    let id = track.id.as_str();
    let start_ms = s.timestamp_ms as i64 - s.position_ms as i64;
    let duration = if s.duration_ms > 0 {
        s.duration_ms
    } else {
        track.duration_ms
    };
    Some(Activity {
        track_id: id.to_owned(),
        title: track.title.clone(),
        artists: track.artist_names(),
        album: track.album.as_ref().map(|a| a.title.clone()),
        start_ms,
        end_ms: (duration > 0).then_some(start_ms + duration as i64),
        image: art
            .map(str::to_owned)
            .or_else(|| is_youtube(id).then(|| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))),
        link: is_youtube(id).then(|| format!("https://music.youtube.com/watch?v={id}")),
        paused,
    })
}

/// Whether a new activity differs enough from the last one to send it: periodic
/// position samples drift by a few ms and are not worth an update.
fn changed(old: Option<&Activity>, new: Option<&Activity>) -> bool {
    match (old, new) {
        (None, None) => false,
        (Some(a), Some(b)) => {
            a.track_id != b.track_id
                || a.title != b.title
                || (a.start_ms - b.start_ms).abs() > SEEK_SLACK_MS
                || a.end_ms.is_some() != b.end_ms.is_some()
                || a.paused != b.paused
                || a.image != b.image
        }
        _ => true,
    }
}

fn refresh(st: &Session, p: &Presence) {
    let now = unix_ms();
    let art_of = |id: &str| p.art.lock().expect("art lock poisoned").get(id).cloned().flatten();
    let mut inner = p.inner.lock().expect("presence lock poisoned");
    let Some(s) = inner.last.clone() else { return };
    let want = match (&inner.current, s.track_id.as_ref()) {
        (Some(c), Some(id)) if c.track.id == *id => desired(&c.track, &s, art_of(id.as_str()).as_deref(), now),
        // Restored at start before this was installed: read it from the library. Never from
        // the player: this runs inside its event callback, under its lock.
        (None, Some(id)) => match st.db.read(|c| mixtape_library::repo::track(c, id)).ok().flatten() {
            Some(t) => {
                let a = desired(&t, &s, art_of(id.as_str()).as_deref(), now);
                inner.current = Some(Current { track: t });
                a
            }
            _ => return,
        },
        // A gapless switch shows its next sample before its announcement: wait for it.
        (Some(_), Some(_)) => return,
        (_, None) => None,
    };
    if changed(inner.sent.as_ref(), want.as_ref()) {
        inner.sent = want.clone();
        drop(inner);
        p.send(Msg::Set(want));
    }
}

pub fn on_player_event(st: &Session, ev: &PlayerEvent) {
    if APP_ID.is_none() {
        return;
    }
    let Some(p) = st.app.try_state::<Presence>() else {
        return;
    };
    if !p.enabled.load(Ordering::Relaxed) {
        return;
    }
    match ev {
        PlayerEvent::State(s) => {
            let was = p.inner.lock().expect("presence lock poisoned").last.replace(s.clone());
            if s.status == Status::Paused && was.is_none_or(|w| w.status != Status::Paused) {
                expire_pause(st);
            }
            // A track restored at start never announces itself: ask for its art here too.
            if let Some(id) = s.track_id.as_ref() {
                fetch_art(st, id.as_str());
            }
        }
        PlayerEvent::TrackChanged { track, .. } => {
            p.inner.lock().expect("presence lock poisoned").current = Some(Current {
                track: track.clone(),
            });
            fetch_art(st, track.id.as_str());
        }
        _ => return,
    }
    refresh(st, &p);
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// One look a little after the pause's grace time: still paused → the status goes.
fn expire_pause(st: &Session) {
    let app = st.app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(PAUSE_KEEP_MS as u64 + 500)).await;
        if let (Some(st), Some(p)) = (app.try_state::<std::sync::Arc<Session>>(), app.try_state::<Presence>()) {
            refresh(&st, &p);
        }
    });
}

/// The album art YouTube Music shows for a song (square, unlike the video thumbnail),
/// asked once per track; the activity is refreshed when it arrives.
fn fetch_art(st: &Session, id: &str) {
    if !is_youtube(id) || !st.yt.is_authenticated() {
        return;
    }
    let Some(p) = st.app.try_state::<Presence>() else { return };
    {
        let mut art = p.art.lock().expect("art lock poisoned");
        if art.contains_key(id) {
            return;
        }
        art.insert(id.to_owned(), None);
    }
    let app = st.app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(async move {
        let Some(st) = app.try_state::<std::sync::Arc<Session>>() else { return };
        let Ok(v) = st.yt.player_meta(&mixtape_core::TrackId::from(id.as_str())).await else { return };
        let url = v["videoDetails"]["thumbnail"]["thumbnails"]
            .as_array()
            .and_then(|t| t.last())
            .and_then(|t| t["url"].as_str())
            // Songs carry square Google-hosted art; videos only their 16:9 frame.
            .filter(|u| u.contains("googleusercontent.com") || u.contains("ggpht.com"))
            .map(mixtape_yt::thumbs::hi_res);
        let Some(url) = url else { return };
        let Some(p) = app.try_state::<Presence>() else { return };
        p.art.lock().expect("art lock poisoned").insert(id, Some(url));
        refresh(&st, &p);
    });
}

/// The setting changed.
pub fn set_enabled(st: &Session, on: bool) {
    let Some(p) = st.app.try_state::<Presence>() else {
        return;
    };
    if p.enabled.swap(on, Ordering::Relaxed) == on || APP_ID.is_none() {
        return;
    }
    if on {
        // Ask the player first: its thread holds the player lock while it calls
        // `on_player_event`, which takes `p.inner` — taking them in the other order deadlocks.
        let player = st.running_player();
        let last = player.as_ref().map(|p| p.state()).unwrap_or_default();
        let current = player.and_then(|p| p.current_track()).map(|track| Current { track });
        {
            let mut inner = p.inner.lock().expect("presence lock poisoned");
            inner.last = Some(last);
            inner.current = current;
            inner.sent = None;
        }
        refresh(st, &p);
    } else {
        let had = p
            .inner
            .lock()
            .expect("presence lock poisoned")
            .sent
            .take()
            .is_some();
        if had || p.tx.get().is_some() {
            p.send(Msg::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Discord's side of the pipe: canned replies to read, everything written is kept.
    struct FakePipe {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl FakePipe {
        fn new(replies: &[(u32, Value)]) -> Self {
            Self {
                input: Cursor::new(replies.iter().flat_map(|(op, v)| encode(*op, v)).collect()),
                output: Vec::new(),
            }
        }

        fn written(&self) -> Vec<(u32, Value)> {
            let mut r = Cursor::new(self.output.clone());
            let mut out = Vec::new();
            while (r.position() as usize) < self.output.len() {
                out.push(read_frame(&mut r).unwrap());
            }
            out
        }
    }

    impl Read for FakePipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for FakePipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn activity() -> Activity {
        Activity {
            track_id: "djV11Xbc914".into(),
            title: "Take On Me".into(),
            artists: "a-ha".into(),
            album: Some("Hunting High and Low".into()),
            start_ms: 1_700_000_000_000,
            end_ms: Some(1_700_000_225_000),
            image: Some("https://i.ytimg.com/vi/djV11Xbc914/hqdefault.jpg".into()),
            link: Some("https://music.youtube.com/watch?v=djV11Xbc914".into()),
            paused: false,
        }
    }

    #[test]
    fn frames_are_little_endian_op_length_json() {
        let bytes = encode(OP_HANDSHAKE, &json!({ "v": 1, "client_id": "123" }));
        let body = br#"{"client_id":"123","v":1}"#;
        assert_eq!(&bytes[..4], &[0, 0, 0, 0]);
        assert_eq!(&bytes[4..8], &(body.len() as u32).to_le_bytes());
        assert_eq!(&bytes[8..], body);
        let (op, v) = read_frame(&mut Cursor::new(encode(OP_FRAME, &json!({ "a": "б" })))).unwrap();
        assert_eq!((op, v), (OP_FRAME, json!({ "a": "б" })));
        // Truncated and oversized frames are errors, not hangs or huge allocations.
        assert!(read_frame(&mut Cursor::new(vec![1, 0, 0, 0, 9, 0, 0, 0, b'{'])).is_err());
        let mut huge = vec![1, 0, 0, 0];
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_frame(&mut Cursor::new(huge)).is_err());
    }

    #[test]
    fn listening_activity_payload() {
        let v = set_activity(42, 7, Some(&activity()));
        assert_eq!(v["cmd"], "SET_ACTIVITY");
        assert_eq!(v["nonce"], "7");
        assert_eq!(v["args"]["pid"], 42);
        let a = &v["args"]["activity"];
        assert_eq!(a["type"], 2);
        assert_eq!(a["details"], "Take On Me");
        assert_eq!(a["state"], "a-ha");
        assert_eq!(
            a["timestamps"],
            json!({ "start": 1_700_000_000_000i64, "end": 1_700_000_225_000i64 })
        );
        assert_eq!(
            a["assets"]["large_image"],
            "https://i.ytimg.com/vi/djV11Xbc914/hqdefault.jpg"
        );
        assert_eq!(a["assets"]["large_text"], "Hunting High and Low");
        assert_eq!(
            a["buttons"][0]["url"],
            "https://music.youtube.com/watch?v=djV11Xbc914"
        );
        // A local file: the app's picture, no button; short text padded to Discord's minimum.
        let local = Activity {
            image: None,
            link: None,
            album: None,
            title: "X".into(),
            end_ms: None,
            ..activity()
        };
        let a = &set_activity(1, 1, Some(&local))["args"]["activity"];
        assert_eq!(a["assets"], json!({ "large_image": "mixtape" }));
        assert!(a.get("buttons").is_none());
        assert_eq!(a["details"].as_str().unwrap().chars().count(), 2);
        assert!(a["timestamps"].get("end").is_none());
        // Clearing sends no activity at all.
        assert!(set_activity(1, 2, None)["args"].get("activity").is_none());
    }

    #[test]
    fn handshake_then_set_and_clear_answering_pings() {
        let ready = json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "v": 1 } });
        let ok = json!({ "cmd": "SET_ACTIVITY", "evt": null, "nonce": "1" });
        let pipe = FakePipe::new(&[
            (OP_FRAME, ready),
            (OP_PING, json!({ "n": 1 })),
            (OP_FRAME, ok.clone()),
            (OP_FRAME, ok),
        ]);
        let mut c = Client::connect(pipe, "1234").unwrap();
        c.set(Some(&activity())).unwrap();
        c.set(None).unwrap();
        let w = c.io.written();
        assert_eq!(w.len(), 4);
        assert_eq!(w[0], (OP_HANDSHAKE, json!({ "v": 1, "client_id": "1234" })));
        assert_eq!(w[1].0, OP_FRAME);
        assert_eq!(w[1].1["args"]["activity"]["details"], "Take On Me");
        assert_eq!(w[2], (OP_PONG, json!({ "n": 1 })));
        assert_eq!(w[3].1["nonce"], "2");
        assert!(w[3].1["args"].get("activity").is_none());
    }

    #[test]
    fn closed_or_refused_connection_is_an_error() {
        let pipe = FakePipe::new(&[(
            OP_CLOSE,
            json!({ "code": 4000, "message": "Invalid Client ID" }),
        )]);
        assert!(Client::connect(pipe, "bad").is_err());
        // Discord went away mid-session: the reply never comes.
        let pipe = FakePipe::new(&[(OP_FRAME, json!({ "evt": "READY" }))]);
        let mut c = Client::connect(pipe, "1").unwrap();
        assert!(c.set(None).is_err());
    }

    #[test]
    fn only_real_changes_are_sent() {
        let a = activity();
        let drift = Activity {
            start_ms: a.start_ms + 300,
            end_ms: a.end_ms.map(|e| e + 300),
            ..a.clone()
        };
        let seek = Activity {
            start_ms: a.start_ms - 60_000,
            ..a.clone()
        };
        let other = Activity {
            track_id: "other000000".into(),
            ..a.clone()
        };
        assert!(!changed(None, None));
        assert!(changed(None, Some(&a)));
        assert!(changed(Some(&a), None));
        assert!(!changed(Some(&a), Some(&drift)));
        assert!(changed(Some(&a), Some(&seek)));
        assert!(changed(Some(&a), Some(&other)));
    }

    #[test]
    fn worker_waits_only_when_it_has_to() {
        let mut w = Worker {
            client_id: "1",
            conn: None,
            want: None,
            shown: None,
            last_try: None,
            last_send: None,
        };
        // Nothing to show and no connection: sleep until a message comes.
        assert_eq!(w.deadline(), None);
        w.want = Some(activity());
        assert!(w.deadline().is_some());
        // Discord not running: the next attempt is a minute after the last one.
        let t = Instant::now();
        w.last_try = Some(t);
        assert_eq!(w.deadline(), Some(t + RETRY_EVERY));
        // Paused while disconnected: no retries at all.
        w.want = None;
        assert_eq!(w.deadline(), None);
    }

    #[test]
    fn activity_follows_position_and_status() {
        use mixtape_core::{ArtistRef, TrackId, TrackState};
        let track = Track {
            id: TrackId::from("djV11Xbc914"),
            title: "Take On Me".into(),
            artists: vec![ArtistRef {
                id: None,
                name: "a-ha".into(),
            }],
            album: None,
            track_no: None,
            duration_ms: 225_000,
            cover_key: None,
            state: TrackState::Remote,
            liked: false,
        };
        let s = PlaybackState {
            track_id: Some(track.id.clone()),
            status: Status::Playing,
            position_ms: 60_000,
            duration_ms: 225_000,
            timestamp_ms: 1_700_000_060_000,
            ..Default::default()
        };
        let now = 1_700_000_060_000;
        let a = desired(&track, &s, None, now).unwrap();
        assert_eq!(
            (a.start_ms, a.end_ms),
            (1_700_000_000_000, Some(1_700_000_225_000))
        );
        assert_eq!(
            a.link.as_deref(),
            Some("https://music.youtube.com/watch?v=djV11Xbc914")
        );
        let paused = PlaybackState { status: Status::Paused, ..s.clone() };
        // A fresh pause keeps the status, marked and without the time bar…
        let a = desired(&track, &paused, Some("https://lh3.googleusercontent.com/x=w544"), now + 1_000).unwrap();
        assert!(a.paused && a.image.as_deref() == Some("https://lh3.googleusercontent.com/x=w544"));
        let v = set_activity(1, 1, Some(&a));
        assert!(v["args"]["activity"].get("timestamps").is_none());
        // …and it goes after the grace time.
        assert!(desired(&track, &paused, None, now + PAUSE_KEEP_MS + 1).is_none());
        let local = Track {
            id: TrackId::from("local1a2b3c4d"),
            ..track
        };
        let a = desired(&local, &s, None, now).unwrap();
        assert!(a.image.is_none() && a.link.is_none());
    }
}
