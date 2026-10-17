//! Last.fm: the desktop sign-in, "now playing", scrobbles and the rules for when a play
//! counts (https://www.last.fm/api/scrobbling).
//!
//! - Sign-in: `auth.getToken`, the user allows the token in the browser
//!   ([`LastFm::auth_url`]), then `auth.getSession` gives the session key.
//! - A play is scrobbled once the track (longer than 30 s) has played for half its length
//!   or 4 minutes, whichever comes first ([`PlayTracker`]); the timestamp is when it
//!   started. Scrobbles wait in `scrobble_outbox` and go out in batches of 50 ([`flush`]).
//! - Every call is signed: `api_sig` = md5 of the parameters sorted by name and
//!   concatenated as name + value, followed by the shared secret ([`sign`]).

use md5::{Digest, Md5};
use mixtape_core::tr;
use mixtape_library::Db;
use mixtape_library::scrobbles::{self, Scrobble};
use serde_json::Value;
use std::time::Duration;

pub const API_BASE: &str = "https://ws.audioscrobbler.com/2.0/";
const AUTH_PAGE: &str = "https://www.last.fm/api/auth/";
const USER_AGENT: &str = "Mixtape/0.1 (https://github.com/Reva1v/mixtape)";
/// Scrobbles per `track.scrobble` call (the API's limit).
pub const BATCH: usize = 50;
/// Batches sent per flush; the rest wait for the next one.
const MAX_BATCHES: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum LastFmError {
    #[error("{}: {}", tr("network error", "помилка мережі", "ошибка сети"), .0)]
    Network(#[from] reqwest::Error),
    #[error("{} {}", tr("Last.fm answered with error", "Last.fm відповів помилкою", "Last.fm ответил ошибкой"), .0)]
    Http(u16),
    #[error("Last.fm: {message} ({code})")]
    Api { code: u32, message: String },
    #[error(
        "{}",
        tr(
            "Last.fm sent an unexpected answer",
            "Last.fm надіслав неочікувану відповідь",
            "Last.fm прислал неожиданный ответ"
        )
    )]
    BadResponse,
}

impl LastFmError {
    /// The session key is no longer valid: the user has to connect again.
    pub fn is_auth(&self) -> bool {
        matches!(self, Self::Api { code: 9, .. })
    }

    /// No answer, or Last.fm is down or throttling: worth retrying later as it is.
    pub fn is_temporary(&self) -> bool {
        matches!(
            self,
            Self::Network(_)
                | Self::Http(_)
                | Self::Api {
                    code: 8 | 11 | 16 | 29,
                    ..
                }
        )
    }

    /// `auth.getSession` before the user allowed access in the browser.
    pub fn not_authorized_yet(&self) -> bool {
        matches!(self, Self::Api { code: 14, .. })
    }

    /// The token expired or is unknown: sign-in starts over.
    pub fn token_dead(&self) -> bool {
        matches!(self, Self::Api { code: 4 | 15, .. })
    }
}

/// The signed-in Last.fm user.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub name: String,
    pub key: String,
}

/// What `track.updateNowPlaying` and `track.scrobble` describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayMeta {
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub duration_ms: u64,
}

impl PlayMeta {
    fn duration_s(&self) -> Option<u32> {
        (self.duration_ms >= 1000).then_some((self.duration_ms / 1000) as u32)
    }
}

/// `api_sig` for a call: parameters sorted by name, each as name + value, then the
/// secret, md5 as lowercase hex. `format` and `callback` are not signed.
pub fn sign(params: &[(String, String)], secret: &str) -> String {
    let mut sorted: Vec<&(String, String)> = params
        .iter()
        .filter(|(k, _)| k != "format" && k != "callback")
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Md5::new();
    for (k, v) in sorted {
        h.update(k.as_bytes());
        h.update(v.as_bytes());
    }
    h.update(secret.as_bytes());
    format!("{:x}", h.finalize())
}

/// `application/x-www-form-urlencoded` (outside async code: the serializer is not `Send`).
fn form(params: &[(String, String)]) -> String {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in params {
        form.append_pair(k, v);
    }
    form.finish()
}

pub struct LastFm {
    http: reqwest::Client,
    base: String,
    key: String,
    secret: String,
}

impl LastFm {
    pub fn new(key: &str, secret: &str) -> reqwest::Result<Self> {
        Self::with_base(API_BASE, key, secret)
    }

    pub fn with_base(base: &str, key: &str, secret: &str) -> reqwest::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            base: base.to_owned(),
            key: key.to_owned(),
            secret: secret.to_owned(),
        })
    }

    /// The page where the user allows Mixtape to scrobble for the token.
    pub fn auth_url(&self, token: &str) -> String {
        let q = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("api_key", &self.key)
            .append_pair("token", token)
            .finish();
        format!("{AUTH_PAGE}?{q}")
    }

    /// All parameters of a signed call, `api_sig` and `format` included.
    fn signed(
        &self,
        method: &str,
        mut params: Vec<(String, String)>,
        sk: Option<&str>,
    ) -> Vec<(String, String)> {
        params.push(("method".into(), method.into()));
        params.push(("api_key".into(), self.key.clone()));
        if let Some(sk) = sk {
            params.push(("sk".into(), sk.into()));
        }
        let sig = sign(&params, &self.secret);
        params.push(("api_sig".into(), sig));
        params.push(("format".into(), "json".into()));
        params
    }

    async fn call(
        &self,
        method: &str,
        params: Vec<(String, String)>,
        sk: Option<&str>,
        post: bool,
    ) -> Result<Value, LastFmError> {
        let encoded = form(&self.signed(method, params, sk));
        let req = if post {
            self.http
                .post(&self.base)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(encoded)
        } else {
            self.http.get(format!("{}?{encoded}", self.base))
        };
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        // Errors come as JSON with any status; only a body without it is an HTTP error.
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            return Err(if status.is_success() {
                LastFmError::BadResponse
            } else {
                LastFmError::Http(status.as_u16())
            });
        };
        if let Some(code) = v.get("error").and_then(Value::as_u64) {
            let message = v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            return Err(LastFmError::Api {
                code: code as u32,
                message,
            });
        }
        if !status.is_success() {
            return Err(LastFmError::Http(status.as_u16()));
        }
        Ok(v)
    }

    /// A fresh token for the sign-in (valid for an hour).
    pub async fn get_token(&self) -> Result<String, LastFmError> {
        let v = self.call("auth.getToken", Vec::new(), None, false).await?;
        v.get("token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(LastFmError::BadResponse)
    }

    /// The session for a token the user has allowed.
    pub async fn get_session(&self, token: &str) -> Result<Session, LastFmError> {
        let v = self
            .call(
                "auth.getSession",
                vec![("token".into(), token.into())],
                None,
                false,
            )
            .await?;
        let s = v.get("session").ok_or(LastFmError::BadResponse)?;
        let field = |k: &str| s.get(k).and_then(Value::as_str).map(str::to_owned);
        Ok(Session {
            name: field("name").ok_or(LastFmError::BadResponse)?,
            key: field("key").ok_or(LastFmError::BadResponse)?,
        })
    }

    pub async fn update_now_playing(&self, sk: &str, m: &PlayMeta) -> Result<(), LastFmError> {
        let mut p = vec![
            ("artist".into(), m.artist.clone()),
            ("track".into(), m.track.clone()),
        ];
        if let Some(a) = m.album.as_ref().filter(|a| !a.is_empty()) {
            p.push(("album".into(), a.clone()));
        }
        if let Some(d) = m.duration_s() {
            p.push(("duration".into(), d.to_string()));
        }
        self.call("track.updateNowPlaying", p, Some(sk), true)
            .await
            .map(drop)
    }

    /// Sends up to [`BATCH`] scrobbles. Returns how many Last.fm accepted; the rest it
    /// ignored (too old, filtered) and will never take.
    pub async fn scrobble(&self, sk: &str, batch: &[Scrobble]) -> Result<u32, LastFmError> {
        let mut p = Vec::with_capacity(batch.len() * 5);
        for (i, s) in batch.iter().take(BATCH).enumerate() {
            p.push((format!("artist[{i}]"), s.artist.clone()));
            p.push((format!("track[{i}]"), s.track.clone()));
            p.push((format!("timestamp[{i}]"), s.started_at.to_string()));
            if let Some(a) = s.album.as_ref().filter(|a| !a.is_empty()) {
                p.push((format!("album[{i}]"), a.clone()));
            }
            if let Some(d) = s.duration_s {
                p.push((format!("duration[{i}]"), d.to_string()));
            }
        }
        let v = self.call("track.scrobble", p, Some(sk), true).await?;
        let attr = v
            .pointer("/scrobbles/@attr")
            .ok_or(LastFmError::BadResponse)?;
        // Numbers arrive as strings or numbers depending on the endpoint's mood.
        let accepted = match attr.get("accepted") {
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
            Some(Value::String(s)) => s.parse().unwrap_or(0),
            _ => 0,
        };
        Ok(accepted as u32)
    }
}

// ---------- outbox ----------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlushReport {
    pub sent: u32,
    /// Kept for a retry.
    pub failed: u32,
    /// Given up after repeated failures or too old for Last.fm.
    pub dropped: u32,
    /// The session key was revoked; nothing more was sent.
    pub auth_expired: bool,
    /// Last.fm could not be reached or was down; the rows wait without counting an attempt.
    pub unreachable: bool,
}

/// Sends queued scrobbles oldest first in batches of [`BATCH`]. Any failure stops the
/// flush. A refusal counts an attempt on the batch; no answer (no network), an outage and
/// a revoked session leave the rows untouched.
pub async fn flush(api: &LastFm, db: &Db, sk: &str, now_s: i64) -> FlushReport {
    let mut report = FlushReport::default();
    match db.write(move |t| scrobbles::drop_expired(t, now_s)) {
        Ok(n) => report.dropped += n as u32,
        Err(e) => {
            tracing::warn!(error = %e, "scrobble outbox unreadable");
            return report;
        }
    }
    for _ in 0..MAX_BATCHES {
        let rows = match db.read(|c| scrobbles::pending(c, BATCH)) {
            Ok(r) if r.is_empty() => break,
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "scrobble outbox unreadable");
                break;
            }
        };
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        let batch: Vec<Scrobble> = rows.into_iter().map(|r| r.scrobble).collect();
        match api.scrobble(sk, &batch).await {
            Ok(_) => {
                report.sent += ids.len() as u32;
                if let Err(e) = db.write(move |t| scrobbles::sent(t, &ids)) {
                    tracing::warn!(error = %e, "scrobble outbox not updated");
                    break;
                }
            }
            Err(e) if e.is_auth() => {
                report.auth_expired = true;
                break;
            }
            Err(e) => {
                tracing::warn!(error = %e, n = ids.len(), "scrobbles not sent");
                report.failed += ids.len() as u32;
                if e.is_temporary() {
                    report.unreachable = true;
                    break;
                }
                match db.write(move |t| scrobbles::failed(t, &ids)) {
                    Ok(n) => report.dropped += n as u32,
                    Err(e) => tracing::warn!(error = %e, "scrobble outbox not updated"),
                }
                break;
            }
        }
    }
    report
}

// ---------- when a play counts ----------

/// Tracks of 30 s or less are never scrobbled.
const MIN_TRACK_MS: u64 = 30_000;
/// A play counts after half the track or this long, whichever comes first.
const ENOUGH_MS: u64 = 4 * 60_000;
/// A longer gap between two samples while playing is not counted (the PC slept, the
/// process stalled); the player reports at least every ~5 s while playing.
const MAX_STEP_MS: u64 = 30_000;

/// How long a track must play to be scrobbled, or `None` when it never is.
pub fn threshold_ms(duration_ms: u64) -> Option<u64> {
    (duration_ms > MIN_TRACK_MS).then(|| (duration_ms / 2).min(ENOUGH_MS))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayAction {
    /// The play started: tell Last.fm what plays now.
    NowPlaying(PlayMeta),
    /// The play counts: queue it.
    Scrobble(Scrobble),
}

struct Play {
    id: String,
    /// `None` for tracks Last.fm cannot take (no artist).
    meta: Option<PlayMeta>,
    /// Unix seconds when it first played.
    started_at: Option<i64>,
    played_ms: u64,
    /// Wall clock of the last sample while playing.
    since: Option<u64>,
    scrobbled: bool,
}

/// Follows the player and decides when to send "now playing" and when a play counts.
/// Time is the time actually played: pauses, buffering and seeks do not add to it.
#[derive(Default)]
pub struct PlayTracker {
    play: Option<Play>,
    playing: bool,
}

impl PlayTracker {
    /// Whether a track is being followed.
    pub fn has_play(&self) -> bool {
        self.play.is_some()
    }

    /// A new track became current (`now_ms`: Unix ms). Closes the previous play.
    pub fn track_changed(
        &mut self,
        id: &str,
        meta: Option<PlayMeta>,
        now_ms: u64,
    ) -> Vec<PlayAction> {
        let mut out = Vec::new();
        self.tick(now_ms, &mut out);
        self.play = Some(Play {
            id: id.to_owned(),
            meta,
            started_at: None,
            played_ms: 0,
            since: None,
            scrobbled: false,
        });
        if self.playing {
            self.resume(now_ms, &mut out);
        }
        out
    }

    /// A player state sample: `playing` only while sound actually comes out.
    pub fn state(
        &mut self,
        id: Option<&str>,
        playing: bool,
        duration_ms: u64,
        now_ms: u64,
    ) -> Vec<PlayAction> {
        let mut out = Vec::new();
        self.tick(now_ms, &mut out);
        // Before a gapless switch is announced the sample may already be the next track's.
        let same = matches!((&self.play, id), (Some(p), Some(id)) if p.id == id);
        if same
            && let Some(m) = self.play.as_mut().and_then(|p| p.meta.as_mut())
            && duration_ms > 0
        {
            m.duration_ms = duration_ms;
        }
        self.playing = playing;
        if playing {
            if same || self.play.as_ref().is_some_and(|p| p.started_at.is_some()) {
                self.resume(now_ms, &mut out);
            }
        } else if let Some(p) = self.play.as_mut() {
            p.since = None;
        }
        out
    }

    fn resume(&mut self, now_ms: u64, out: &mut Vec<PlayAction>) {
        let Some(p) = self.play.as_mut() else { return };
        p.since = Some(now_ms);
        if p.started_at.is_none() {
            p.started_at = Some((now_ms / 1000) as i64);
            if let Some(m) = &p.meta {
                out.push(PlayAction::NowPlaying(m.clone()));
            }
        }
    }

    /// Adds the time played since the last sample and checks the rule.
    fn tick(&mut self, now_ms: u64, out: &mut Vec<PlayAction>) {
        let Some(p) = self.play.as_mut() else { return };
        if let Some(s) = p.since {
            let step = now_ms.saturating_sub(s);
            if step <= MAX_STEP_MS {
                p.played_ms += step;
            }
            p.since = Some(now_ms);
        }
        if p.scrobbled {
            return;
        }
        let (Some(m), Some(started_at)) = (&p.meta, p.started_at) else {
            return;
        };
        if threshold_ms(m.duration_ms).is_some_and(|t| p.played_ms >= t) {
            p.scrobbled = true;
            out.push(PlayAction::Scrobble(Scrobble {
                artist: m.artist.clone(),
                track: m.track.clone(),
                album: m.album.clone(),
                duration_s: m.duration_s(),
                started_at,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(k, val)| (k.to_string(), val.to_string()))
            .collect()
    }

    fn md5_hex(s: &str) -> String {
        format!("{:x}", Md5::digest(s.as_bytes()))
    }

    #[test]
    fn md5_matches_known_vectors() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            md5_hex("The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }

    #[test]
    fn signature_sorts_params_and_skips_format() {
        let p = pairs(&[
            ("token", "T"),
            ("method", "auth.getSession"),
            ("api_key", "K"),
            ("format", "json"),
        ]);
        assert_eq!(
            sign(&p, "S"),
            md5_hex("api_keyKmethodauth.getSessiontokenTS")
        );
        // Batch fields sort byte-wise: `album[0]` before `api_key`, `artist[0]` after it.
        let p = pairs(&[
            ("artist[0]", "a-ha"),
            ("album[0]", "Hunting"),
            ("api_key", "K"),
            ("sk", "SK"),
        ]);
        assert_eq!(
            sign(&p, "S"),
            md5_hex("album[0]Huntingapi_keyKartist[0]a-haskSKS")
        );
        // Non-ASCII is signed as UTF-8.
        let p = pairs(&[("track", "Кино")]);
        assert_eq!(sign(&p, "S"), md5_hex("trackКиноS"));
    }

    fn meta(duration_ms: u64) -> PlayMeta {
        PlayMeta {
            artist: "a-ha".into(),
            track: "Take On Me".into(),
            album: Some("Hunting High and Low".into()),
            duration_ms,
        }
    }

    fn scrobbles(actions: &[PlayAction]) -> Vec<i64> {
        actions
            .iter()
            .filter_map(|a| match a {
                PlayAction::Scrobble(s) => Some(s.started_at),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn thresholds() {
        assert_eq!(threshold_ms(30_000), None);
        assert_eq!(threshold_ms(31_000), Some(15_500));
        assert_eq!(threshold_ms(200_000), Some(100_000));
        assert_eq!(threshold_ms(600_000), Some(240_000));
        assert_eq!(threshold_ms(0), None);
    }

    #[test]
    fn scrobbles_after_half_of_the_time_actually_played() {
        let mut t = PlayTracker::default();
        let t0 = 1_700_000_000_000;
        assert!(t.track_changed("a", Some(meta(200_000)), t0).is_empty());
        // Starts playing: "now playing", timestamp = start.
        let a = t.state(Some("a"), true, 200_000, t0 + 1_000);
        assert_eq!(a, vec![PlayAction::NowPlaying(meta(200_000))]);
        let mut now = t0 + 1_000;
        let mut got = Vec::new();
        // 60 s of play in 5 s samples.
        for _ in 0..12 {
            now += 5_000;
            got.extend(t.state(Some("a"), true, 200_000, now));
        }
        // Paused for 10 minutes: does not count.
        got.extend(t.state(Some("a"), false, 200_000, now));
        now += 600_000;
        got.extend(t.state(Some("a"), true, 200_000, now));
        assert!(scrobbles(&got).is_empty());
        // A seek does not count either: only wall time while playing does.
        for _ in 0..7 {
            now += 5_000;
            got.extend(t.state(Some("a"), true, 200_000, now));
        }
        assert!(scrobbles(&got).is_empty(), "95 s played of 100 s needed");
        now += 5_000;
        got.extend(t.state(Some("a"), true, 200_000, now));
        assert_eq!(scrobbles(&got), vec![(t0 + 1_000) as i64 / 1000]);
        // Once per play.
        now += 5_000;
        assert!(t.state(Some("a"), true, 200_000, now).is_empty());
    }

    #[test]
    fn four_minutes_are_enough_and_short_tracks_never_count() {
        let mut t = PlayTracker::default();
        let mut now = 1_000_000;
        t.state(None, true, 0, now);
        // Already playing when the track arrives (gapless): "now playing" at once.
        let a = t.track_changed("long", Some(meta(1_200_000)), now);
        assert!(matches!(a.as_slice(), [PlayAction::NowPlaying(_)]));
        let mut got = Vec::new();
        for _ in 0..48 {
            now += 5_000;
            got.extend(t.state(Some("long"), true, 1_200_000, now));
        }
        assert_eq!(scrobbles(&got).len(), 1);

        let mut got = t.track_changed("short", Some(meta(25_000)), now);
        for _ in 0..5 {
            now += 5_000;
            got.extend(t.state(Some("short"), true, 25_000, now));
        }
        assert!(scrobbles(&got).is_empty());
        // No artist: nothing at all.
        let got = t.track_changed("x", None, now);
        assert!(got.is_empty());
    }

    #[test]
    fn leaving_a_track_closes_its_play_and_gapless_samples_wait_for_the_switch() {
        let mut t = PlayTracker::default();
        let mut now = 5_000_000;
        t.track_changed("a", Some(meta(60_000)), now);
        t.state(Some("a"), true, 60_000, now);
        for _ in 0..5 {
            now += 5_000;
            assert!(scrobbles(&t.state(Some("a"), true, 60_000, now)).is_empty());
        }
        // 25 s played; the next track's first sample arrives before its announcement and
        // carries its own length, which must not change the rule for "a".
        now += 4_000;
        assert!(scrobbles(&t.state(Some("b"), true, 900_000, now)).is_empty());
        now += 1_000;
        let a = t.track_changed("b", Some(meta(900_000)), now);
        assert_eq!(scrobbles(&a), vec![5_000]);
        assert!(matches!(a.last(), Some(PlayAction::NowPlaying(_))));
        // A huge gap (sleep) is not counted.
        now += 3_600_000;
        assert!(scrobbles(&t.state(Some("b"), true, 900_000, now)).is_empty());
    }

    /// A local Last.fm stand-in: answers each request with the next canned body and logs
    /// the request lines and bodies.
    async fn fake_lastfm(
        replies: Vec<(&'static str, &'static str)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/2.0/", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        tokio::spawn(async move {
            let mut replies = replies.into_iter();
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let line = head.lines().next().unwrap_or_default().to_owned();
                let body = String::from_utf8_lossy(&buf[head_end..head_end + len]).into_owned();
                seen.lock().unwrap().push(format!("{line}\n{body}"));
                let (status, reply) = replies
                    .next()
                    .unwrap_or(("500 Internal Server Error", "oops"));
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (base, log)
    }

    fn decode(form: &str) -> Vec<(String, String)> {
        url::form_urlencoded::parse(form.as_bytes())
            .into_owned()
            .collect()
    }

    #[tokio::test]
    async fn desktop_sign_in() {
        let (base, log) = fake_lastfm(vec![
            ("200 OK", r#"{"token":"tok1"}"#),
            ("403 Forbidden", r#"{"error":14,"message":"Unauthorized Token - This token has not been authorized"}"#),
            ("200 OK", r#"{"session":{"name":"listener","key":"sk1","subscriber":0}}"#),
        ])
        .await;
        let api = LastFm::with_base(&base, "KEY", "SECRET").unwrap();
        let token = api.get_token().await.unwrap();
        assert_eq!(token, "tok1");
        assert_eq!(
            api.auth_url(&token),
            "https://www.last.fm/api/auth/?api_key=KEY&token=tok1"
        );
        let err = api.get_session(&token).await.unwrap_err();
        assert!(err.not_authorized_yet(), "{err:?}");
        let s = api.get_session(&token).await.unwrap();
        assert_eq!(
            s,
            Session {
                name: "listener".into(),
                key: "sk1".into()
            }
        );

        let log = log.lock().unwrap().clone();
        let first = log[0].lines().next().unwrap();
        assert!(first.starts_with("GET /2.0/?"), "{first}");
        let query = first
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split_once('?')
            .unwrap()
            .1;
        let params = decode(query);
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("method").as_deref(), Some("auth.getToken"));
        assert_eq!(get("format").as_deref(), Some("json"));
        assert_eq!(
            get("api_sig"),
            Some(md5_hex("api_keyKEYmethodauth.getTokenSECRET"))
        );
        assert!(log[2].contains("token=tok1"));
    }

    #[tokio::test]
    async fn now_playing_is_a_signed_post() {
        let (base, log) = fake_lastfm(vec![("200 OK", r#"{"nowplaying":{}}"#)]).await;
        let api = LastFm::with_base(&base, "KEY", "SECRET").unwrap();
        api.update_now_playing("SK", &meta(225_400)).await.unwrap();
        let req = log.lock().unwrap()[0].clone();
        let (line, body) = req.split_once('\n').unwrap();
        assert!(line.starts_with("POST /2.0/ "));
        let mut params = decode(body);
        let sig = params
            .iter()
            .position(|(k, _)| k == "api_sig")
            .map(|i| params.remove(i).1)
            .unwrap();
        assert_eq!(sig, sign(&params, "SECRET"));
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("method"), Some("track.updateNowPlaying"));
        assert_eq!(get("duration"), Some("225"));
        assert_eq!(get("sk"), Some("SK"));
        assert_eq!(get("album"), Some("Hunting High and Low"));
    }

    fn queued(db: &Db, n: i64) {
        db.write(move |t| {
            for i in 0..n {
                scrobbles::queue(
                    t,
                    &Scrobble {
                        artist: "a-ha".into(),
                        track: format!("Song {i}"),
                        album: None,
                        duration_s: Some(200),
                        started_at: 1_000_000 + i,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    }

    const OK: &str = r#"{"scrobbles":{"@attr":{"accepted":50,"ignored":0},"scrobble":[]}}"#;

    #[tokio::test]
    async fn outbox_goes_out_in_batches_of_fifty() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        queued(&db, 120);
        let (base, log) = fake_lastfm(vec![("200 OK", OK), ("200 OK", OK), ("200 OK", OK)]).await;
        let api = LastFm::with_base(&base, "KEY", "SECRET").unwrap();
        let r = flush(&api, &db, "SK", 1_000_100).await;
        assert_eq!(
            r,
            FlushReport {
                sent: 120,
                ..Default::default()
            }
        );
        let log = log.lock().unwrap().clone();
        assert_eq!(log.len(), 3);
        let first = decode(log[0].split_once('\n').unwrap().1);
        let count = |p: &[(String, String)]| {
            p.iter()
                .filter(|(k, _)| k.starts_with("timestamp["))
                .count()
        };
        assert_eq!(count(&first), 50);
        assert!(first.contains(&("timestamp[0]".into(), "1000000".into())));
        assert!(first.contains(&("track[49]".into(), "Song 49".into())));
        assert_eq!(count(&decode(log[2].split_once('\n').unwrap().1)), 20);
        assert_eq!(db.read(scrobbles::oldest_attempts).unwrap(), None);
    }

    #[tokio::test]
    async fn failures_keep_rows_and_revoked_session_stops() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        queued(&db, 60);
        let (base, log) = fake_lastfm(vec![
            (
                "503 Service Unavailable",
                r#"{"error":16,"message":"Temporarily unavailable"}"#,
            ),
            (
                "400 Bad Request",
                r#"{"error":6,"message":"Invalid parameters"}"#,
            ),
            ("200 OK", r#"{"error":9,"message":"Invalid session key"}"#),
            ("200 OK", OK),
            ("200 OK", OK),
        ])
        .await;
        let api = LastFm::with_base(&base, "KEY", "SECRET").unwrap();
        // Outage: the flush stops, the rows wait as they are.
        let r = flush(&api, &db, "SK", 1_000_100).await;
        assert_eq!((r.sent, r.failed, r.unreachable), (0, 50, true));
        assert_eq!(db.read(scrobbles::oldest_attempts).unwrap(), Some(0));
        // Refused: the batch counts an attempt.
        let r = flush(&api, &db, "SK", 1_000_100).await;
        assert_eq!(
            (r.sent, r.failed, r.unreachable, r.auth_expired),
            (0, 50, false, false)
        );
        assert_eq!(db.read(scrobbles::oldest_attempts).unwrap(), Some(1));
        // Revoked session: nothing changes.
        let r = flush(&api, &db, "SK", 1_000_100).await;
        assert!(r.auth_expired);
        assert_eq!(db.read(|c| scrobbles::pending(c, 100)).unwrap().len(), 60);
        // Back: everything goes, the retried batch included.
        let r = flush(&api, &db, "SK", 1_000_100).await;
        assert_eq!(r.sent, 60);
        assert_eq!(log.lock().unwrap().len(), 5);
        // Rows older than two weeks are dropped without a request.
        queued(&db, 1);
        let r = flush(&api, &db, "SK", 1_000_000 + scrobbles::MAX_AGE_S + 10).await;
        assert_eq!((r.sent, r.dropped), (0, 1));
        assert_eq!(log.lock().unwrap().len(), 5);
        // No network: the rows wait without counting an attempt.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("http://{}/2.0/", closed.local_addr().unwrap());
        drop(closed);
        let api = LastFm::with_base(&dead, "KEY", "SECRET").unwrap();
        queued(&db, 2);
        let r = flush(&api, &db, "SK", 1_000_100).await;
        assert!(r.unreachable && r.failed == 2 && r.sent == 0);
        assert_eq!(db.read(scrobbles::oldest_attempts).unwrap(), Some(0));
    }
}
