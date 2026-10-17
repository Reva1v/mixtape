//! Matching a whole list on YouTube Music and saving it as a local playlist.
//!
//! One search (the Songs filter) per track, at most [`MAX_IN_FLIGHT`] at once and paced by
//! a [`Pacer`] (short pauses between starts, the usual backoff once YouTube throttles). A
//! track without a match is searched once more by its title alone (artists written in
//! another script). Matched songs are stored as `remote` catalog rows, like search results:
//! nothing is downloaded until the user asks.

use super::SourceTrack;
use super::matching::{best_match, query_title};
use crate::sources;
use mixtape_artwork::CoverStore;
use mixtape_core::{PlaylistId, TrackId};
use mixtape_library::{Db, playlists};
use mixtape_yt::YtError;
use mixtape_yt::api::YtTrack;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::limits::{Outcome, Pacer, PacerConfig};
use mixtape_yt::search::SearchKind;
use serde::Serialize;
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use ts_rs::TS;

/// Searches running at the same time.
pub const MAX_IN_FLIGHT: usize = 3;
/// Tries per search when YouTube throttles or the network hiccups.
const ATTEMPTS: usize = 3;

/// Pacing for the searches: three at once, 0.2–0.6 s between starts, the default backoff.
pub fn pacer() -> Pacer {
    Pacer::with_config(PacerConfig {
        max_parallel: MAX_IN_FLIGHT,
        pause_min: Duration::from_millis(200),
        pause_max: Duration::from_millis(600),
        ..PacerConfig::default()
    })
}

/// YouTube Music song search; `InnerTube` in the app, a fake in tests.
pub trait SongSearch: Send + Sync + 'static {
    fn songs(&self, q: &str) -> impl Future<Output = Result<Vec<YtTrack>, YtError>> + Send;
}

impl SongSearch for InnerTube {
    async fn songs(&self, q: &str) -> Result<Vec<YtTrack>, YtError> {
        Ok(self.search(q, SearchKind::Tracks).await?.tracks)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Yt(#[from] YtError),
}

/// What the summary dialog shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpotifyImportResult {
    /// The new local playlist; absent when nothing matched.
    #[ts(optional)]
    pub playlist_id: Option<PlaylistId>,
    pub title: String,
    pub matched: u32,
    /// Tracks looked for (what was read, not what Spotify has).
    pub total: u32,
    /// In source order.
    pub not_found: Vec<SourceTrack>,
    /// Present when Spotify gave only a part of the list.
    #[ts(optional)]
    pub cut: Option<SpotifyCut>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpotifyCut {
    /// Tracks read.
    pub read: u32,
    /// Tracks in the playlist, when Spotify said.
    #[ts(optional)]
    pub total: Option<u32>,
}

/// Search queries for a track, best first.
pub fn queries(t: &SourceTrack) -> Vec<String> {
    let title = query_title(&t.title);
    let title = if title.is_empty() {
        t.title.trim().to_owned()
    } else {
        title
    };
    let artists: Vec<&str> = t.artists.iter().take(2).map(String::as_str).collect();
    if artists.is_empty() {
        return vec![title];
    }
    vec![format!("{} {title}", artists.join(" ")), title]
}

async fn search_paced<S: SongSearch>(
    s: &S,
    pacer: &Pacer,
    cancel: &AtomicBool,
    q: &str,
) -> Result<Vec<YtTrack>, MatchError> {
    let mut last = None;
    for _ in 0..ATTEMPTS {
        if cancel.load(Ordering::SeqCst) {
            return Err(MatchError::Cancelled);
        }
        let _permit = pacer.acquire().await;
        if cancel.load(Ordering::SeqCst) {
            return Err(MatchError::Cancelled);
        }
        match s.songs(q).await {
            Ok(v) => {
                pacer.report(Outcome::Ok);
                return Ok(v);
            }
            Err(YtError::Auth) => return Err(YtError::Auth.into()),
            Err(e) => {
                pacer.report(if matches!(e, YtError::RateLimited) {
                    Outcome::RateLimited
                } else {
                    Outcome::OtherError
                });
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or(YtError::RateLimited).into())
}

/// The best YouTube Music song for one track, `None` when nothing scores high enough.
pub async fn find<S: SongSearch>(
    s: &S,
    pacer: &Pacer,
    cancel: &AtomicBool,
    t: &SourceTrack,
) -> Result<Option<YtTrack>, MatchError> {
    for q in queries(t) {
        let cands = search_paced(s, pacer, cancel, &q).await?;
        if let Some((c, score)) = best_match(t, &cands) {
            tracing::debug!(query = %q, found = %c.title, score, "spotify match");
            return Ok(Some(c.clone()));
        }
    }
    Ok(None)
}

/// Matches every track, in source order. `progress(done, total)` follows each finished
/// track. A track whose searches failed counts as not found, unless they all failed (then
/// that error is returned); an expired account stops everything.
pub async fn match_all<S: SongSearch>(
    search: Arc<S>,
    tracks: &[SourceTrack],
    pacer: Arc<Pacer>,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(usize, usize),
) -> Result<Vec<Option<YtTrack>>, MatchError> {
    let total = tracks.len();
    let mut out: Vec<Option<YtTrack>> = vec![None; total];
    let mut set = tokio::task::JoinSet::new();
    // Tracks go in order: one holds its slot through its retry query, so progress moves
    // steadily instead of every first query running before any second one.
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT));
    for (i, t) in tracks.iter().cloned().enumerate() {
        let (search, pacer, cancel, slots) =
            (search.clone(), pacer.clone(), cancel.clone(), slots.clone());
        set.spawn(async move {
            let Ok(_slot) = slots.acquire_owned().await else {
                return (i, Err(MatchError::Cancelled));
            };
            (i, find(&*search, &pacer, &cancel, &t).await)
        });
    }
    let (mut done, mut failed) = (0, 0);
    let mut last_error = None;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((i, Ok(found))) => out[i] = found,
            Ok((_, Err(MatchError::Cancelled))) => return Err(MatchError::Cancelled),
            Ok((_, Err(MatchError::Yt(YtError::Auth)))) => {
                cancel.store(true, Ordering::SeqCst);
                return Err(YtError::Auth.into());
            }
            Ok((_, Err(MatchError::Yt(e)))) => {
                tracing::warn!(error = %e, "spotify: search failed");
                failed += 1;
                last_error = Some(e);
            }
            Err(e) => {
                tracing::warn!(error = %e, "spotify: search task failed");
                failed += 1;
            }
        }
        done += 1;
        progress(done, total);
    }
    if total > 0
        && failed == total
        && let Some(e) = last_error
    {
        return Err(e.into());
    }
    Ok(out)
}

/// Stores the matched songs as catalog rows and creates the local playlist with them in
/// source order. Returns `None` (and creates nothing) when nothing matched.
pub async fn save_playlist(
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    title: &str,
    found: &[Option<YtTrack>],
) -> mixtape_library::Result<Option<PlaylistId>> {
    let order: Vec<TrackId> = found.iter().flatten().map(|t| t.id.clone()).collect();
    if order.is_empty() {
        return Ok(None);
    }
    let mut seen = HashSet::new();
    let unique: Vec<YtTrack> = found
        .iter()
        .flatten()
        .filter(|t| seen.insert(t.id.clone()))
        .cloned()
        .collect();
    sources::store_tracks(http, db, covers, unique).await?;
    let title = title.to_owned();
    let id = db.write(move |t| {
        let id = playlists::create(t, &title)?;
        playlists::add(t, &id, &order)?;
        Ok(id)
    })?;
    Ok(Some(id))
}

/// The summary for the dialog.
pub fn summary(
    title: &str,
    tracks: &[SourceTrack],
    found: &[Option<YtTrack>],
    playlist_id: Option<PlaylistId>,
    cut: Option<SpotifyCut>,
) -> SpotifyImportResult {
    let not_found: Vec<SourceTrack> = tracks
        .iter()
        .zip(found)
        .filter(|(_, f)| f.is_none())
        .map(|(t, _)| t.clone())
        .collect();
    SpotifyImportResult {
        playlist_id,
        title: title.to_owned(),
        matched: (tracks.len() - not_found.len()) as u32,
        total: tracks.len() as u32,
        not_found,
        cut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_core::ArtistRef;
    use mixtape_yt::api::VideoKind;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    fn src(artist: &str, title: &str, ms: u64) -> SourceTrack {
        SourceTrack {
            title: title.into(),
            artists: vec![artist.into()],
            duration_ms: Some(ms),
        }
    }

    fn yt(id: &str, artist: &str, title: &str, ms: u64) -> YtTrack {
        YtTrack {
            id: TrackId::from(id),
            title: title.into(),
            artists: vec![ArtistRef {
                id: None,
                name: artist.into(),
            }],
            album: None,
            duration_ms: ms,
            thumbnail: None,
            kind: VideoKind::Audio,
            track_no: None,
        }
    }

    /// Answers from a table; counts concurrent calls; fails queries listed in `fail`.
    struct Fake {
        answers: HashMap<String, Vec<YtTrack>>,
        fail: Mutex<HashMap<String, YtError>>,
        queries: Mutex<Vec<String>>,
        running: AtomicUsize,
        peak: AtomicUsize,
    }

    impl Fake {
        fn new(answers: &[(&str, Vec<YtTrack>)]) -> Self {
            Self {
                answers: answers
                    .iter()
                    .map(|(q, v)| ((*q).to_owned(), v.clone()))
                    .collect(),
                fail: Mutex::new(HashMap::new()),
                queries: Mutex::new(Vec::new()),
                running: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
            }
        }
    }

    impl SongSearch for Fake {
        async fn songs(&self, q: &str) -> Result<Vec<YtTrack>, YtError> {
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            self.queries.lock().unwrap().push(q.to_owned());
            tokio::time::sleep(Duration::from_secs(2)).await;
            self.running.fetch_sub(1, Ordering::SeqCst);
            if let Some(e) = self.fail.lock().unwrap().remove(q) {
                return Err(e);
            }
            Ok(self.answers.get(q).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn builds_queries() {
        let t = SourceTrack {
            title: "Panama - 2015 Remaster".into(),
            artists: vec!["Van Halen".into(), "B".into(), "C".into()],
            duration_ms: None,
        };
        assert_eq!(queries(&t), ["Van Halen B Panama", "Panama"]);
        let bare = SourceTrack {
            title: "Song".into(),
            artists: vec![],
            duration_ms: None,
        };
        assert_eq!(queries(&bare), ["Song"]);
    }

    #[tokio::test(start_paused = true)]
    async fn matches_in_order_three_at_a_time() {
        let tracks: Vec<SourceTrack> = (0..9)
            .map(|i| src("Artist", &format!("Song {i}"), 200_000))
            .collect();
        let mut answers: Vec<(String, Vec<YtTrack>)> = (0..9)
            .filter(|i| i % 3 != 2)
            .map(|i| {
                (
                    format!("Artist Song {i}"),
                    vec![yt(
                        &format!("v{i}"),
                        "Artist",
                        &format!("Song {i}"),
                        201_000,
                    )],
                )
            })
            .collect();
        // Song 2 is found only by its title alone (the artist is written differently).
        answers.push(("Song 2".into(), vec![yt("v2", "Артист", "Song 2", 200_500)]));
        let refs: Vec<(&str, Vec<YtTrack>)> = answers
            .iter()
            .map(|(q, v)| (q.as_str(), v.clone()))
            .collect();
        let fake = Arc::new(Fake::new(&refs));
        let seen = Mutex::new(Vec::new());
        let found = match_all(
            fake.clone(),
            &tracks,
            Arc::new(pacer()),
            Arc::new(AtomicBool::new(false)),
            |d, t| seen.lock().unwrap().push((d, t)),
        )
        .await
        .unwrap();
        let ids: Vec<Option<&str>> = found
            .iter()
            .map(|f| f.as_ref().map(|t| t.id.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                Some("v0"),
                Some("v1"),
                Some("v2"),
                Some("v3"),
                Some("v4"),
                None,
                Some("v6"),
                Some("v7"),
                None
            ]
        );
        assert_eq!(fake.peak.load(Ordering::SeqCst), MAX_IN_FLIGHT);
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.len(), 9);
        assert_eq!(seen.last(), Some(&(9, 9)));

        let s = summary("Mix", &tracks, &found, None, None);
        assert_eq!((s.matched, s.total), (7, 9));
        let missing: Vec<&str> = s.not_found.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(missing, ["Song 5", "Song 8"]);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_after_rate_limit_and_stops_on_auth() {
        let tracks = vec![src("A", "One", 100_000)];
        let fake = Arc::new(Fake::new(&[("A One", vec![yt("v1", "A", "One", 100_000)])]));
        fake.fail
            .lock()
            .unwrap()
            .insert("A One".into(), YtError::RateLimited);
        let found = match_all(
            fake.clone(),
            &tracks,
            Arc::new(pacer()),
            Arc::new(AtomicBool::new(false)),
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(found[0].as_ref().map(|t| t.id.as_str()), Some("v1"));
        assert_eq!(
            fake.queries.lock().unwrap().len(),
            2,
            "the throttled search ran again"
        );

        let fake = Arc::new(Fake::new(&[]));
        fake.fail
            .lock()
            .unwrap()
            .insert("A One".into(), YtError::Auth);
        let err = match_all(
            fake,
            &tracks,
            Arc::new(pacer()),
            Arc::new(AtomicBool::new(false)),
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, MatchError::Yt(YtError::Auth)));
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_stops_early() {
        let tracks: Vec<SourceTrack> = (0..30).map(|i| src("A", &format!("S{i}"), 1)).collect();
        let fake = Arc::new(Fake::new(&[]));
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let err = match_all(
            fake.clone(),
            &tracks,
            Arc::new(pacer()),
            cancel,
            move |done, _| {
                if done == 2 {
                    c2.store(true, Ordering::SeqCst);
                }
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, MatchError::Cancelled));
        let n = fake.queries.lock().unwrap().len();
        assert!(n < 20, "{n} searches ran");
    }

    #[tokio::test(start_paused = true)]
    async fn all_failing_is_an_error() {
        let tracks = vec![src("A", "One", 1)];
        let fake = Arc::new(Fake::new(&[]));
        fake.fail
            .lock()
            .unwrap()
            .insert("A One".into(), YtError::Network("down".into()));
        // One failure, then the retry answers: not an error.
        assert!(
            match_all(
                fake.clone(),
                &tracks,
                Arc::new(pacer()),
                Arc::new(AtomicBool::new(false)),
                |_, _| {}
            )
            .await
            .is_ok()
        );

        struct Down;
        impl SongSearch for Down {
            async fn songs(&self, _: &str) -> Result<Vec<YtTrack>, YtError> {
                Err(YtError::Network("down".into()))
            }
        }
        let err = match_all(
            Arc::new(Down),
            &tracks,
            Arc::new(pacer()),
            Arc::new(AtomicBool::new(false)),
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, MatchError::Yt(YtError::Network(_))));
    }
}
