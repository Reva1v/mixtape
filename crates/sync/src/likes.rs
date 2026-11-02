//! Sends queued likes (`like_outbox`) to YouTube Music once there is a network and an
//! account.

use mixtape_artwork::CoverStore;
use mixtape_core::TrackId;
use mixtape_library::{Db, likes, repo, ytm};
use mixtape_yt::YtError;
use mixtape_yt::api::{YtMusic, YtTrack};
use std::collections::HashSet;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Rows taken per flush; the rest wait for the next one.
const BATCH: usize = 500;

/// The YouTube Music call behind a like (`like/like`, `like/removelike`).
pub trait LikeApi: Send + Sync {
    fn set_like(
        &self,
        id: &TrackId,
        liked: bool,
    ) -> impl Future<Output = Result<(), YtError>> + Send;
}

impl LikeApi for mixtape_yt::innertube::InnerTube {
    async fn set_like(&self, id: &TrackId, liked: bool) -> Result<(), YtError> {
        mixtape_yt::innertube::InnerTube::set_like(self, id, liked).await
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlushReport {
    pub sent: u32,
    /// Kept for a retry (network, throttling, unexpected errors).
    pub failed: u32,
    /// Dropped because YouTube refuses the track.
    pub dropped: u32,
    /// The account needs reconnecting; nothing more was sent.
    pub auth_expired: bool,
    /// Network trouble or throttling stopped it: another try right now would only fail again.
    pub interrupted: bool,
}

/// Sends queued actions oldest first. Network trouble and throttling stop the flush and
/// keep everything for later, without counting against the row: a like survives any time
/// offline. An expired login stops it too; an unavailable track drops its row; other errors
/// keep the row, count an attempt and move on.
pub async fn flush<A: LikeApi>(api: &A, db: &Db) -> FlushReport {
    let mut report = FlushReport::default();
    let pending = match db.read(|c| likes::pending_likes(c, BATCH)) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "like outbox unreadable");
            return report;
        }
    };
    for row in pending {
        let id = row.id;
        let (done, stop) = match api.set_like(&row.track_id, row.liked).await {
            Ok(()) => {
                report.sent += 1;
                note_sent(&row.track_id, row.liked);
                (true, false)
            }
            Err(YtError::Unavailable(why)) => {
                tracing::info!(track = %row.track_id, %why, "like dropped");
                report.dropped += 1;
                (true, false)
            }
            Err(YtError::Auth) => {
                report.auth_expired = true;
                return report;
            }
            Err(e @ (YtError::Network(_) | YtError::RateLimited)) => {
                tracing::info!(track = %row.track_id, error = %e, "like waits for the network");
                report.failed += 1;
                report.interrupted = true;
                return report;
            }
            Err(e) => {
                tracing::warn!(track = %row.track_id, error = %e, "like not sent");
                report.failed += 1;
                (false, false)
            }
        };
        let res = db.write(move |t| {
            if done {
                likes::like_sent(t, id).map(|()| false)
            } else {
                likes::like_failed(t, id)
            }
        });
        match res {
            Ok(true) => {
                tracing::warn!(track = %row.track_id, attempts = likes::MAX_ATTEMPTS, "like given up after repeated failures");
                report.dropped += 1;
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(error = %e, "like outbox not updated");
                break;
            }
        }
        if stop {
            break;
        }
    }
    report
}

/// A like sent this recently wins over what a read reports: YouTube Music's list can lag
/// behind the send, and the mark must not flip back.
const RECENT: Duration = Duration::from_secs(60);
static SENT: Mutex<Vec<(TrackId, bool, Instant)>> = Mutex::new(Vec::new());

fn note_sent(id: &TrackId, liked: bool) {
    let mut sent = SENT.lock().unwrap_or_else(|e| e.into_inner());
    sent.retain(|(t, _, at)| t != id && at.elapsed() < RECENT);
    sent.push((id.clone(), liked, Instant::now()));
}

/// `read` with the recently sent likes laid over it; a new like goes to the top.
pub fn with_recent(read: &[TrackId]) -> Vec<TrackId> {
    let sent = SENT.lock().unwrap_or_else(|e| e.into_inner());
    let fresh: Vec<_> = sent.iter().filter(|(_, _, at)| at.elapsed() < RECENT).collect();
    let mut out: Vec<TrackId> = fresh.iter().filter(|(t, liked, _)| *liked && !read.contains(t)).map(|(t, _, _)| t.clone()).collect();
    out.extend(read.iter().filter(|id| !fresh.iter().any(|(t, liked, _)| t == *id && !*liked)).cloned());
    out
}

/// The liked list as a check reads it.
pub trait LikedList: Send + Sync {
    fn liked_fingerprint(&self) -> impl Future<Output = Result<u64, YtError>> + Send;
    fn liked_tracks(&self) -> impl Future<Output = Result<Vec<YtTrack>, YtError>> + Send;
}

impl LikedList for mixtape_yt::innertube::InnerTube {
    async fn liked_fingerprint(&self) -> Result<u64, YtError> {
        mixtape_yt::innertube::InnerTube::liked_fingerprint(self).await
    }
    async fn liked_tracks(&self) -> Result<Vec<YtTrack>, YtError> {
        YtMusic::liked_tracks(self).await
    }
}

/// The whole list is read at least this often, whatever the fingerprint says.
const FULL_READ: Duration = Duration::from_secs(10 * 60);
/// An empty read is refused while this device holds more likes than this: a page YouTube
/// reshaped parses as empty, and believing it would take off every like.
const EMPTY_GUARD: i64 = 5;

/// What the last check saw; one per app.
#[derive(Debug, Default)]
pub struct CheckState {
    seen: Mutex<Option<(u64, Instant)>>,
}

/// Brings the local likes up to the account: one light request when nothing changed, the
/// whole list when something did. Returns the likes that changed here. Tracks this device
/// has not met are added first, so they show in its liked list.
pub async fn check<A: LikedList>(
    api: &A,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    state: &CheckState,
) -> Result<Vec<(TrackId, bool)>, YtError> {
    let fingerprint = api.liked_fingerprint().await?;
    let seen = *state.seen.lock().unwrap_or_else(|e| e.into_inner());
    if seen.is_some_and(|(f, at)| f == fingerprint && at.elapsed() < FULL_READ) {
        return Ok(Vec::new());
    }
    let tracks = api.liked_tracks().await?;
    let storage = |e: mixtape_library::Error| YtError::Parse(format!("likes not stored: {e}"));
    if tracks.is_empty() {
        let held: i64 = db
            .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM tracks WHERE liked = 1 AND length(id) = 11 AND id NOT LIKE 'local%'", [], |r| r.get(0))?))
            .map_err(storage)?;
        if held > EMPTY_GUARD {
            tracing::warn!(held, "liked list read as empty: ignored");
            return Ok(Vec::new());
        }
    }
    let ids: Vec<TrackId> = tracks.iter().map(|t| t.id.clone()).collect();
    let known = ids.clone();
    let known: HashSet<TrackId> = db.read(move |c| repo::existing_track_ids(c, &known)).map_err(storage)?;
    let missing: Vec<YtTrack> = tracks.into_iter().filter(|t| !known.contains(&t.id)).collect();
    if !missing.is_empty() {
        tracing::info!(n = missing.len(), "liked tracks new to this device");
        crate::sources::store_tracks(http, db, covers, missing).await.map_err(storage)?;
    }
    let ids = with_recent(&ids);
    let changes = db
        .write(move |t| {
            let before = repo::liked_track_ids(t)?;
            ytm::apply_remote_likes(t, &ids)?;
            let after = repo::liked_track_ids(t)?;
            let mut changes: Vec<(TrackId, bool)> = after.difference(&before).map(|id| (id.clone(), true)).collect();
            changes.extend(before.difference(&after).map(|id| (id.clone(), false)));
            Ok(changes)
        })
        .map_err(storage)?;
    *state.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some((fingerprint, Instant::now()));
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_library::repo::{self, TrackMeta};
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeApi {
        errors: Mutex<HashMap<String, YtError>>,
        calls: Mutex<Vec<(String, bool)>>,
    }

    impl FakeApi {
        fn failing(pairs: Vec<(&str, YtError)>) -> Self {
            let api = Self::default();
            *api.errors.lock().unwrap() =
                pairs.into_iter().map(|(k, e)| (k.to_owned(), e)).collect();
            api
        }
    }

    impl LikeApi for FakeApi {
        async fn set_like(&self, id: &TrackId, liked: bool) -> Result<(), YtError> {
            self.calls.lock().unwrap().push((id.0.clone(), liked));
            match self.errors.lock().unwrap().remove(id.as_str()) {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }

    fn setup(actions: &[(&'static str, bool)]) -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let actions = actions.to_vec();
        db.write(move |t| {
            for (id, liked) in actions {
                let meta = TrackMeta {
                    id: TrackId::from(id),
                    title: id.into(),
                    artists: Vec::new(),
                    album: None,
                    track_no: None,
                    duration_ms: 1000,
                    cover_key: None,
                };
                repo::upsert_track(t, &meta)?;
                likes::queue_like(t, &TrackId::from(id), liked)?;
            }
            Ok(())
        })
        .unwrap();
        (dir, db)
    }

    fn pending(db: &Db) -> Vec<(String, bool, u32)> {
        db.read(|c| likes::pending_likes(c, 100))
            .unwrap()
            .into_iter()
            .map(|p| (p.track_id.0, p.liked, p.attempts))
            .collect()
    }

    #[tokio::test]
    async fn sends_collapsed_actions_oldest_first() {
        let (_d, db) = setup(&[("a", true), ("b", true), ("a", false)]);
        let api = FakeApi::default();
        let report = flush(&api, &db).await;
        assert_eq!(
            report,
            FlushReport {
                sent: 2,
                ..Default::default()
            }
        );
        assert_eq!(
            *api.calls.lock().unwrap(),
            vec![("b".into(), true), ("a".into(), false)]
        );
        assert!(pending(&db).is_empty());
    }

    #[tokio::test]
    async fn network_error_stops_and_keeps_the_rest() {
        let (_d, db) = setup(&[("a", true), ("b", true), ("c", true)]);
        let api = FakeApi::failing(vec![("b", YtError::Network("offline".into()))]);
        let report = flush(&api, &db).await;
        assert_eq!(
            report,
            FlushReport {
                sent: 1,
                failed: 1,
                interrupted: true,
                ..Default::default()
            }
        );
        assert_eq!(
            pending(&db),
            vec![("b".into(), true, 0), ("c".into(), true, 0)]
        );
        // Next time the network is back.
        assert_eq!(flush(&api, &db).await.sent, 2);
        assert!(pending(&db).is_empty());
    }

    #[tokio::test]
    async fn auth_stops_and_unavailable_drops() {
        let (_d, db) = setup(&[("a", true), ("b", false), ("c", true)]);
        let api = FakeApi::failing(vec![
            ("a", YtError::Unavailable("видео удалено".into())),
            ("b", YtError::Auth),
        ]);
        let report = flush(&api, &db).await;
        assert_eq!(
            report,
            FlushReport {
                dropped: 1,
                auth_expired: true,
                ..Default::default()
            }
        );
        assert_eq!(
            pending(&db),
            vec![("b".into(), false, 0), ("c".into(), true, 0)]
        );
    }

    #[tokio::test]
    async fn a_row_failing_too_often_is_dropped() {
        let (_d, db) = setup(&[("a", true), ("b", true)]);
        let max = likes::MAX_ATTEMPTS;
        db.write(move |t| {
            t.execute("UPDATE like_outbox SET attempts = ?1 WHERE track_id = 'a'", [max - 1])?;
            Ok(())
        })
        .unwrap();
        let api = FakeApi::failing(vec![("a", YtError::Parse("bad".into()))]);
        let report = flush(&api, &db).await;
        assert_eq!(report, FlushReport { sent: 1, failed: 1, dropped: 1, ..Default::default() });
        assert!(pending(&db).is_empty());
    }

    #[tokio::test]
    async fn offline_for_any_time_never_loses_a_like() {
        let (_d, db) = setup(&[("a", true)]);
        for _ in 0..likes::MAX_ATTEMPTS * 3 {
            let api = FakeApi::failing(vec![("a", YtError::Network("offline".into()))]);
            assert!(flush(&api, &db).await.interrupted);
        }
        assert_eq!(pending(&db), vec![("a".into(), true, 0)]);
    }

    struct FakeList {
        fingerprint: Mutex<u64>,
        tracks: Mutex<Vec<YtTrack>>,
        reads: Mutex<u32>,
    }

    impl FakeList {
        fn new(ids: &[&str]) -> Self {
            Self { fingerprint: Mutex::new(1), tracks: Mutex::new(ids.iter().map(|i| yt(i)).collect()), reads: Mutex::new(0) }
        }
        fn set(&self, ids: &[&str]) {
            *self.fingerprint.lock().unwrap() += 1;
            *self.tracks.lock().unwrap() = ids.iter().map(|i| yt(i)).collect();
        }
    }

    impl LikedList for FakeList {
        async fn liked_fingerprint(&self) -> Result<u64, YtError> {
            Ok(*self.fingerprint.lock().unwrap())
        }
        async fn liked_tracks(&self) -> Result<Vec<YtTrack>, YtError> {
            *self.reads.lock().unwrap() += 1;
            Ok(self.tracks.lock().unwrap().clone())
        }
    }

    fn yt(id: &str) -> YtTrack {
        YtTrack {
            id: TrackId::from(id),
            title: id.into(),
            artists: Vec::new(),
            album: None,
            duration_ms: 1000,
            thumbnail: None,
            kind: mixtape_yt::api::VideoKind::Audio,
            track_no: None,
        }
    }

    fn check_env() -> (tempfile::TempDir, Db, CoverStore, reqwest::Client) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let covers = CoverStore::new(dir.path().join("covers"));
        (dir, db, covers, reqwest::Client::new())
    }

    fn liked(db: &Db, id: &str) -> Option<bool> {
        let id = TrackId::from(id);
        db.read(move |c| repo::track(c, &id)).unwrap().map(|t| t.liked)
    }

    #[tokio::test]
    async fn check_reads_the_list_only_when_it_changed_and_adds_new_tracks() {
        let (_d, db, covers, http) = check_env();
        let state = CheckState::default();
        let api = FakeList::new(&["checkaaaaaa"]);
        let changed = check(&api, &http, &db, &covers, &state).await.unwrap();
        assert_eq!(changed, vec![(TrackId::from("checkaaaaaa"), true)]);
        assert_eq!(liked(&db, "checkaaaaaa"), Some(true));

        assert!(check(&api, &http, &db, &covers, &state).await.unwrap().is_empty());
        assert_eq!(*api.reads.lock().unwrap(), 1);

        api.set(&["checkbbbbbb"]);
        let mut changed = check(&api, &http, &db, &covers, &state).await.unwrap();
        changed.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        assert_eq!(changed, vec![(TrackId::from("checkaaaaaa"), false), (TrackId::from("checkbbbbbb"), true)]);
        assert_eq!(*api.reads.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn a_just_sent_like_survives_a_stale_read() {
        let (_d, db, covers, http) = check_env();
        note_sent(&TrackId::from("recentliked"), true);
        note_sent(&TrackId::from("recentunlik"), false);
        let api = FakeList::new(&["recentliked", "recentunlik"]);
        check(&api, &http, &db, &covers, &CheckState::default()).await.unwrap();
        assert_eq!(liked(&db, "recentliked"), Some(true));
        assert_eq!(liked(&db, "recentunlik"), Some(false));
        let read = [TrackId::from("otherlikeda")];
        let merged = with_recent(&read);
        assert!(merged.contains(&TrackId::from("recentliked")) && merged.contains(&read[0]));
    }

    #[tokio::test]
    async fn a_list_read_as_empty_does_not_wipe_the_likes() {
        let (_d, db, covers, http) = check_env();
        let ids = ["emptyaaaaaa", "emptybbbbbb", "emptycccccc", "emptydddddd", "emptyeeeeee", "emptyffffff"];
        let state = CheckState::default();
        let api = FakeList::new(&ids);
        check(&api, &http, &db, &covers, &state).await.unwrap();
        api.set(&[]);
        assert!(check(&api, &http, &db, &covers, &state).await.unwrap().is_empty());
        assert_eq!(liked(&db, "emptyaaaaaa"), Some(true));
        // A few likes may really all be gone.
        api.set(&["emptyaaaaaa"]);
        check(&api, &http, &db, &covers, &state).await.unwrap();
        api.set(&[]);
        check(&api, &http, &db, &covers, &state).await.unwrap();
        assert_eq!(liked(&db, "emptyaaaaaa"), Some(false));
    }
}
