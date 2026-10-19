//! Song lyrics from lrclib.net (research.md R9), cached in `lyrics_cache`.
//!
//! Lookup: `/api/get` by title, artist, album and duration; when that misses,
//! `/api/search` and the result closest in duration (within ±2 s). Synced LRC wins over
//! plain text. "Not found" is cached too and looked up again after a week.

use mixtape_core::{AppError, ErrorKind, Track, tr, trf};
use mixtape_library::{Db, lyrics as cache};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use mixtape_yt::innertube::{BodyError, MAX_JSON_BYTES, read_limited};
use ts_rs::TS;

const BASE: &str = "https://lrclib.net";
const USER_AGENT: &str = "Mixtape/0.1 (https://github.com/Reva1v/mixtape)";
/// How long a "not found" stays trusted.
const RECHECK_MS: i64 = 7 * 24 * 3600 * 1000;
/// Search results further than this from the track's length are other recordings.
const DURATION_SLACK_S: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LyricsSource {
    Lrclib,
}

/// One timed line; `text` is empty for an instrumental gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LyricLine {
    #[ts(type = "number")]
    pub ms: u64,
    pub text: String,
}

/// Lyrics of one track: timed `lines` when `synced`, otherwise `plain` text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Lyrics {
    pub synced: bool,
    pub lines: Vec<LyricLine>,
    #[ts(optional)]
    pub plain: Option<String>,
    /// The provider marks the track as having no words.
    pub instrumental: bool,
    pub source: LyricsSource,
}

#[derive(Debug, thiserror::Error)]
pub enum LyricsError {
    #[error("{}: {}", tr("network error", "помилка мережі", "ошибка сети"), .0)]
    Network(#[from] reqwest::Error),
    #[error("{} {}", tr("the lyrics server answered with error", "сервер текстів відповів помилкою", "сервер текстов ответил ошибкой"), .0)]
    Http(u16),
    #[error("{0}")]
    Library(#[from] mixtape_library::Error),
}

impl From<LyricsError> for AppError {
    fn from(e: LyricsError) -> Self {
        let kind = match e {
            LyricsError::Library(_) => ErrorKind::Storage,
            _ => ErrorKind::Network,
        };
        AppError::new(
            kind,
            trf!("Couldn't load the lyrics: {e}", "Не вдалося завантажити текст: {e}", "Не удалось загрузить текст: {e}"),
        )
    }
}

// ---------- lrclib ----------

/// What lrclib returns for one record (only the fields used).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    plain_lyrics: Option<String>,
    #[serde(default)]
    synced_lyrics: Option<String>,
}

/// A lookup result as it is cached: `(synced, body)`; an empty body is an instrumental.
type Found = (bool, String);

impl Record {
    fn found(self) -> Option<Found> {
        let some = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
        if self.instrumental {
            Some((false, String::new()))
        } else if let Some(lrc) = some(self.synced_lyrics) {
            Some((true, lrc))
        } else {
            some(self.plain_lyrics).map(|p| (false, p))
        }
    }
}

/// The track fields lrclib matches on.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: u64,
}

impl Query {
    /// The first credited artist only: lrclib stores the primary one.
    pub fn from_track(t: &Track) -> Self {
        let artist = t
            .artists
            .first()
            .map(|a| a.name.as_str())
            .unwrap_or_default();
        Self {
            title: t.title.clone(),
            artist: artist.strip_suffix(" - Topic").unwrap_or(artist).to_owned(),
            album: t
                .album
                .as_ref()
                .map(|a| a.title.clone())
                .filter(|a| !a.is_empty()),
            duration_ms: t.duration_ms,
        }
    }
}

/// lrclib.net client.
#[derive(Clone)]
pub struct Lrclib {
    http: reqwest::Client,
    base: String,
}

impl Lrclib {
    pub fn new() -> reqwest::Result<Self> {
        Self::with_base(BASE)
    }

    pub fn with_base(base: &str) -> reqwest::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    /// GETs `path` with `params`; `None` on 404.
    async fn get(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<Option<Vec<u8>>, LyricsError> {
        let url = url::Url::parse_with_params(&format!("{}{path}", self.base), params)
            .expect("lrclib URL is valid");
        let resp = self.http.get(url).send().await?;
        match resp.status().as_u16() {
            200 => match read_limited(resp, MAX_JSON_BYTES).await {
                Ok(body) => Ok(Some(body)),
                Err(BodyError::Http(e)) => Err(e.into()),
                // 413 Content Too Large: a lyrics answer is never anywhere near the cap.
                Err(BodyError::TooLarge(_)) => Err(LyricsError::Http(413)),
            },
            404 => Ok(None),
            code => Err(LyricsError::Http(code)),
        }
    }

    /// Finds lyrics for `q`; `None` when lrclib has none.
    pub async fn find(&self, q: &Query) -> Result<Option<Found>, LyricsError> {
        let secs = (q.duration_ms as f64 / 1000.0).round().to_string();
        if q.duration_ms > 0
            && let Some(album) = &q.album
        {
            let params = [
                ("track_name", q.title.as_str()),
                ("artist_name", &q.artist),
                ("album_name", album),
                ("duration", &secs),
            ];
            if let Some(body) = self.get("/api/get", &params).await?
                && let Ok(rec) = serde_json::from_slice::<Record>(&body)
                && let Some(found) = rec.found()
            {
                return Ok(Some(found));
            }
        }
        let mut titles = vec![q.title.clone()];
        let clean = clean_title(&q.title);
        if clean != q.title && !clean.is_empty() {
            titles.push(clean);
        }
        for title in titles {
            let params = [("track_name", title.as_str()), ("artist_name", &q.artist)];
            let Some(body) = self.get("/api/search", &params).await? else {
                continue;
            };
            let records: Vec<Record> = serde_json::from_slice(&body).unwrap_or_default();
            if let Some(found) = pick(records, q.duration_ms) {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
}

/// The closest search result in duration (within the slack); synced beats plain.
fn pick(records: Vec<Record>, duration_ms: u64) -> Option<Found> {
    let target = duration_ms as f64 / 1000.0;
    let mut best: Option<(bool, f64, Found)> = None;
    for rec in records {
        let diff = match rec.duration {
            Some(d) if duration_ms > 0 => (d - target).abs(),
            _ if duration_ms > 0 => continue,
            _ => 0.0,
        };
        if diff > DURATION_SLACK_S {
            continue;
        }
        let Some(found) = rec.found() else { continue };
        let better = match &best {
            None => true,
            Some((synced, d, _)) => (found.0 && !synced) || (found.0 == *synced && diff < *d),
        };
        if better {
            best = Some((found.0, diff, found));
        }
    }
    best.map(|b| b.2)
}

/// Drops video decorations like "(Official Video)" or "[Lyric Video]" from a title.
fn clean_title(title: &str) -> String {
    const NOISE: [&str; 7] = ["official", "video", "audio", "lyric", "visuali", "клип", "кліп"];
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    while let Some(start) = rest.find(['(', '[']) {
        let close = if rest[start..].starts_with('(') {
            ')'
        } else {
            ']'
        };
        let Some(len) = rest[start..].find(close) else {
            break;
        };
        let inner = rest[start + 1..start + len].to_lowercase();
        out.push_str(&rest[..start]);
        if !NOISE.iter().any(|n| inner.contains(n)) {
            out.push_str(&rest[start..=start + len]);
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------- LRC ----------

/// Parses LRC: `[mm:ss.xx]` / `[mm:ss]` / `[mm:ss.xxx]` stamps (several per line),
/// `[offset:±ms]`, enhanced-LRC word stamps (dropped). Metadata tags are ignored;
/// stamped blank lines stay as gaps, repeated gaps collapse. Sorted by time.
pub fn parse_lrc(src: &str) -> Vec<LyricLine> {
    let mut offset = 0i64;
    let mut lines: Vec<(i64, String)> = Vec::new();
    for raw in src.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(tag) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            let (inner, after) = tag;
            if let Some(ms) = parse_stamp(inner) {
                stamps.push(ms);
            } else if let Some(v) = inner.strip_prefix("offset:") {
                offset = v.trim().trim_start_matches('+').parse().unwrap_or(0);
            }
            rest = after.trim_start();
        }
        if stamps.is_empty() {
            continue;
        }
        let text = strip_word_stamps(rest);
        for ms in stamps {
            lines.push((ms, text.clone()));
        }
    }
    lines.sort_by_key(|l| l.0);
    let mut out: Vec<LyricLine> = Vec::with_capacity(lines.len());
    for (ms, text) in lines {
        if text.is_empty() && out.last().is_none_or(|l| l.text.is_empty()) {
            continue;
        }
        // A positive offset shows the words earlier.
        out.push(LyricLine {
            ms: (ms - offset).max(0) as u64,
            text,
        });
    }
    while out.last().is_some_and(|l| l.text.is_empty()) {
        out.pop();
    }
    out
}

/// `mm:ss`, `mm:ss.f…` or `mm:ss:ff` → ms.
fn parse_stamp(s: &str) -> Option<i64> {
    let (min, rest) = s.split_once(':')?;
    let (sec, frac) = match rest.split_once(['.', ':']) {
        Some((s, f)) => (s, Some(f)),
        None => (rest, None),
    };
    let digits = |v: &str| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit());
    if !digits(min) || !digits(sec) || frac.is_some_and(|f| !digits(f)) {
        return None;
    }
    let mut ms = min.parse::<i64>().ok()? * 60_000 + sec.parse::<i64>().ok()? * 1000;
    if let Some(f) = frac {
        let f3: String = f.chars().chain("00".chars()).take(3).collect();
        ms += f3.parse::<i64>().ok()?;
    }
    Some(ms)
}

/// Removes `<mm:ss.xx>` word timings from a line's text.
fn strip_word_stamps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        match rest[start..].find('>') {
            Some(len) if parse_stamp(&rest[start + 1..start + len]).is_some() => {
                out.push_str(&rest[..start]);
                rest = &rest[start + len + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Turns a cache row into what the UI shows; `None` for "not found".
pub fn from_cache(synced: bool, body: Option<&str>) -> Option<Lyrics> {
    let body = body?;
    let make = |synced, lines, plain, instrumental| Lyrics {
        synced,
        lines,
        plain,
        instrumental,
        source: LyricsSource::Lrclib,
    };
    if body.trim().is_empty() {
        return Some(make(false, Vec::new(), None, true));
    }
    if synced {
        let lines = parse_lrc(body);
        if lines.iter().any(|l| !l.text.is_empty()) {
            return Some(make(true, lines, None, false));
        }
    }
    Some(make(false, Vec::new(), Some(body.trim().to_owned()), false))
}

/// Whether a cache row can be served without asking lrclib again.
fn is_fresh(row: &cache::CachedLyrics, now_ms: i64) -> bool {
    row.body.is_some() || now_ms - row.fetched_at < RECHECK_MS
}

/// Lyrics for `track`: from the cache, else from lrclib (and then cached). A network
/// failure with an expired "not found" in the cache still answers "not found".
pub async fn get(client: &Lrclib, db: &Db, track: &Track) -> Result<Option<Lyrics>, LyricsError> {
    let id = track.id.clone();
    let cached = db.read(|c| cache::get(c, &id))?;
    if let Some(row) = &cached
        && is_fresh(row, mixtape_library::repo::now_ms())
    {
        return Ok(from_cache(row.synced, row.body.as_deref()));
    }
    let found = match client.find(&Query::from_track(track)).await {
        Ok(found) => found,
        Err(e) if cached.is_some() => {
            tracing::debug!(error = %e, "lyrics lookup failed, keeping the cached miss");
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let (synced, body) = match found {
        Some((s, b)) => (s, Some(b)),
        None => (false, None),
    };
    let lyrics = from_cache(synced, body.as_deref());
    db.write(move |t| cache::put(t, &id, synced, body.as_deref()))?;
    Ok(lyrics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_core::{AlbumId, AlbumRef, ArtistRef, TrackId, TrackState};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn line(ms: u64, text: &str) -> LyricLine {
        LyricLine {
            ms,
            text: text.into(),
        }
    }

    #[test]
    fn stamps() {
        assert_eq!(parse_stamp("01:02.34"), Some(62_340));
        assert_eq!(parse_stamp("01:02.5"), Some(62_500));
        assert_eq!(parse_stamp("01:02.345"), Some(62_345));
        assert_eq!(parse_stamp("01:02"), Some(62_000));
        assert_eq!(parse_stamp("01:02:34"), Some(62_340));
        assert_eq!(parse_stamp("ar:Someone"), None);
        assert_eq!(parse_stamp("01:xx"), None);
    }

    #[test]
    fn parses_lrc() {
        let src = "[ar:a-ha]\n[ti:Take On Me]\n\n[00:12.50]We're talking away\n\
                   [00:20.00][01:10.00]Take on me\n[00:30]\n[00:31.00]\n\
                   [00:40.10]<00:40.10>I don't <00:41.00>know\n[00:50.00]   \n";
        assert_eq!(
            parse_lrc(src),
            vec![
                line(12_500, "We're talking away"),
                line(20_000, "Take on me"),
                line(30_000, ""),
                line(40_100, "I don't know"),
                line(50_000, ""),
                line(70_000, "Take on me"),
            ]
        );
    }

    #[test]
    fn applies_offset() {
        assert_eq!(
            parse_lrc("[offset:+500]\n[00:01.00]a\n[00:00.20]b"),
            vec![line(0, "b"), line(500, "a")]
        );
        assert_eq!(
            parse_lrc("[offset:-250]\n[00:01.00]a"),
            vec![line(1250, "a")]
        );
    }

    #[test]
    fn plain_text_is_not_lrc() {
        assert!(parse_lrc("Just words\nno stamps").is_empty());
        let l = from_cache(true, Some("Just words\nno stamps")).unwrap();
        assert!(!l.synced);
        assert_eq!(l.plain.as_deref(), Some("Just words\nno stamps"));
        assert!(from_cache(false, None).is_none());
        assert!(from_cache(false, Some("")).unwrap().instrumental);
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Take On Me (Official Video)"), "Take On Me");
        assert_eq!(clean_title("Song [Lyric Video] (Remix)"), "Song (Remix)");
        assert_eq!(clean_title("Песня (Официальный клип)"), "Песня");
        assert_eq!(clean_title("Пісня (Офіційний кліп)"), "Пісня");
        assert_eq!(clean_title("Plain"), "Plain");
    }

    fn rec(duration: f64, synced: Option<&str>, plain: Option<&str>) -> Record {
        Record {
            duration: Some(duration),
            instrumental: false,
            plain_lyrics: plain.map(Into::into),
            synced_lyrics: synced.map(Into::into),
        }
    }

    #[test]
    fn picks_closest_synced() {
        let recs = vec![
            rec(200.0, Some("[00:01.00]far"), None),
            rec(181.5, None, Some("plain")),
            rec(178.5, Some("[00:01.00]near"), None),
            rec(180.5, Some("[00:01.00]nearest"), None),
        ];
        assert_eq!(
            pick(recs, 180_000),
            Some((true, "[00:01.00]nearest".into()))
        );
        assert_eq!(
            pick(vec![rec(181.0, None, Some("p"))], 180_000),
            Some((false, "p".into()))
        );
        assert_eq!(pick(vec![rec(190.0, Some("x"), None)], 180_000), None);
    }

    #[test]
    fn not_found_expires_after_a_week() {
        let row = |body: Option<&str>, at| cache::CachedLyrics {
            synced: false,
            body: body.map(Into::into),
            fetched_at: at,
        };
        let now = 10 * RECHECK_MS;
        assert!(is_fresh(&row(None, now - RECHECK_MS + 1), now));
        assert!(!is_fresh(&row(None, now - RECHECK_MS), now));
        assert!(is_fresh(&row(Some("x"), 0), now));
    }

    /// Fake lrclib: `/api/get` 404s, `/api/search` answers `search`. Logs request paths.
    async fn fake_lrclib(search: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let (status, body) = if path.starts_with("/api/search") {
                    ("200 OK", search)
                } else {
                    ("404 Not Found", "{}")
                };
                seen.lock().unwrap().push(path);
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (base, log)
    }

    fn track() -> Track {
        Track {
            id: TrackId::from("t1"),
            title: "Take On Me (Official Video)".into(),
            artists: vec![ArtistRef {
                id: None,
                name: "a-ha".into(),
            }],
            album: Some(AlbumRef {
                id: AlbumId::from("MPRE1"),
                title: "Hunting High and Low".into(),
            }),
            track_no: None,
            duration_ms: 225_000,
            cover_key: None,
            state: TrackState::Remote,
            liked: false,
        }
    }

    #[tokio::test]
    async fn falls_back_to_search_and_caches() {
        let (base, log) = fake_lrclib(
            r#"[{"duration":226.0,"instrumental":false,"syncedLyrics":"[00:01.00]Take on me"}]"#,
        )
        .await;
        let client = Lrclib::with_base(&base).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let l = get(&client, &db, &track()).await.unwrap().unwrap();
        assert!(l.synced);
        assert_eq!(l.lines, vec![line(1000, "Take on me")]);
        let paths = log.lock().unwrap().clone();
        assert!(paths[0].starts_with("/api/get?") && paths[0].contains("duration=225"));
        assert!(paths[1].starts_with("/api/search?"));
        // Second call comes from the cache.
        get(&client, &db, &track()).await.unwrap().unwrap();
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn caches_not_found() {
        let (base, log) = fake_lrclib("[]").await;
        let client = Lrclib::with_base(&base).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        assert!(get(&client, &db, &track()).await.unwrap().is_none());
        // get + search with the raw title + search with the cleaned one.
        assert_eq!(log.lock().unwrap().len(), 3);
        assert!(get(&client, &db, &track()).await.unwrap().is_none());
        assert_eq!(log.lock().unwrap().len(), 3);
        let row = db
            .read(|c| cache::get(c, &TrackId::from("t1")))
            .unwrap()
            .unwrap();
        assert!(row.body.is_none());
    }
}
