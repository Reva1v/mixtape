//! Sends queued likes (`like_outbox`) to YouTube Music once there is a network and an
//! account.

use mixtape_core::TrackId;
use mixtape_library::{Db, likes};
use mixtape_yt::YtError;
use std::future::Future;

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlushReport {
    pub sent: u32,
    /// Kept for a retry (network, throttling, unexpected errors).
    pub failed: u32,
    /// Dropped because YouTube refuses the track.
    pub dropped: u32,
    /// The account needs reconnecting; nothing more was sent.
    pub auth_expired: bool,
}

/// Sends queued actions oldest first. Network trouble and throttling stop the flush and
/// keep the rest for later; an expired login stops it too; an unavailable track drops its
/// row; other errors keep the row and move on.
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
            Err(e) => {
                tracing::warn!(track = %row.track_id, error = %e, "like not sent");
                report.failed += 1;
                (
                    false,
                    matches!(e, YtError::Network(_) | YtError::RateLimited),
                )
            }
        };
        let res = db.write(move |t| {
            if done {
                likes::like_sent(t, id)
            } else {
                likes::like_failed(t, id)
            }
        });
        if let Err(e) = res {
            tracing::warn!(error = %e, "like outbox not updated");
            break;
        }
        if stop {
            break;
        }
    }
    report
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
                ..Default::default()
            }
        );
        assert_eq!(
            pending(&db),
            vec![("b".into(), true, 1), ("c".into(), true, 0)]
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
}
