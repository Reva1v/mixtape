//! Download queue stored in `download_jobs` (data-model.md).
//!
//! Transitions: `queued → running → done | failed`, `running/queued ⇄ paused`,
//! `failed → queued` (retry) and `running → queued` after a restart. At most one active
//! (`queued`/`running`/`paused`) job exists per track; the partial unique index
//! `download_jobs_active` enforces it, and the functions here never violate it.
//! Functions that change one job return whether the transition applied.

use crate::Result;
use crate::repo::now_ms;
use mixtape_core::TrackId;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

/// One row of `download_jobs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: i64,
    pub track_id: TrackId,
    /// Where it was queued from: a sync source kind, `url`, `autosave` or `user`.
    pub source: String,
    pub status: JobStatus,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub attempts: u32,
    pub error: Option<String>,
}

/// Job counts per status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobsSummary {
    pub queued: u32,
    pub running: u32,
    pub paused: u32,
    pub done: u32,
    pub failed: u32,
}

impl JobsSummary {
    /// Jobs that still have to finish.
    pub fn pending(&self) -> u32 {
        self.queued + self.running + self.paused
    }
}

/// A failed job with what the "failed downloads" list shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedJob {
    pub id: i64,
    pub track_id: TrackId,
    pub title: String,
    /// Artist names joined with ", ".
    pub artists: String,
    pub error: String,
    pub attempts: u32,
}

const JOB_COLS: &str = "id, track_id, source, status, bytes_done, bytes_total, attempts, error";

fn job_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let status: String = r.get(3)?;
    Ok(Job {
        id: r.get(0)?,
        track_id: TrackId(r.get(1)?),
        source: r.get(2)?,
        status: JobStatus::parse(&status).unwrap_or(JobStatus::Failed),
        bytes_done: r.get::<_, i64>(4)?.max(0) as u64,
        bytes_total: r.get::<_, Option<i64>>(5)?.map(|v| v.max(0) as u64),
        attempts: r.get(6)?,
        error: r.get(7)?,
    })
}

/// Queues a download of `track_id`. Does nothing (returns `None`) when the track is
/// already `downloaded` or has an active job. A previous failed job for the track is
/// revived instead of adding a second row, so the failed list does not keep stale
/// entries. The track must exist in `tracks`.
pub fn enqueue(conn: &Connection, track_id: &TrackId, source: &str) -> Result<Option<i64>> {
    let id = track_id.as_str();
    let blocked: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tracks WHERE id = ?1 AND state = 'downloaded')
             OR EXISTS(SELECT 1 FROM download_jobs
                       WHERE track_id = ?1 AND status IN ('queued', 'running', 'paused'))",
        [id],
        |r| r.get(0),
    )?;
    if blocked {
        return Ok(None);
    }
    let failed: Option<i64> = conn
        .query_row(
            "SELECT max(id) FROM download_jobs WHERE track_id = ?1 AND status = 'failed'",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(job) = failed {
        conn.execute(
            "UPDATE download_jobs SET status = 'queued', source = ?2, error = NULL, updated_at = ?3
             WHERE id = ?1",
            params![job, source, now_ms()],
        )?;
        return Ok(Some(job));
    }
    conn.execute(
        "INSERT INTO download_jobs(track_id, source, status, updated_at) VALUES (?1, ?2, 'queued', ?3)",
        params![id, source, now_ms()],
    )?;
    Ok(Some(conn.last_insert_rowid()))
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Job>> {
    Ok(conn
        .query_row(
            &format!("SELECT {JOB_COLS} FROM download_jobs WHERE id = ?1"),
            [id],
            job_from_row,
        )
        .optional()?)
}

/// The oldest queued job, i.e. the next one to start.
pub fn next_queued(conn: &Connection) -> Result<Option<Job>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {JOB_COLS} FROM download_jobs WHERE status = 'queued' ORDER BY id LIMIT 1"
            ),
            [],
            job_from_row,
        )
        .optional()?)
}

/// Tracks of the next `limit` queued jobs, in the order they start.
pub fn next_queued_tracks(conn: &Connection, limit: usize) -> Result<Vec<TrackId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT track_id FROM download_jobs WHERE status = 'queued' ORDER BY id LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |r| Ok(TrackId(r.get(0)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Jobs currently downloading (for the progress line).
pub fn running(conn: &Connection) -> Result<Vec<Job>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {JOB_COLS} FROM download_jobs WHERE status = 'running' ORDER BY id"
    ))?;
    let rows = stmt
        .query_map([], job_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn transition(conn: &Connection, id: i64, from: &str, set: &str) -> Result<bool> {
    let n = conn.execute(
        &format!(
            "UPDATE download_jobs SET {set}, updated_at = ?2 WHERE id = ?1 AND status IN ({from})"
        ),
        params![id, now_ms()],
    )?;
    Ok(n > 0)
}

/// `queued → running`.
pub fn mark_running(conn: &Connection, id: i64) -> Result<bool> {
    transition(conn, id, "'queued'", "status = 'running', error = NULL")
}

/// Stores download progress of a running job.
pub fn progress(
    conn: &Connection,
    id: i64,
    bytes_done: u64,
    bytes_total: Option<u64>,
) -> Result<bool> {
    let n = conn.execute(
        "UPDATE download_jobs SET bytes_done = ?2, bytes_total = COALESCE(?3, bytes_total),
            updated_at = ?4
         WHERE id = ?1 AND status = 'running'",
        params![
            id,
            bytes_done as i64,
            bytes_total.map(|v| v as i64),
            now_ms()
        ],
    )?;
    Ok(n > 0)
}

/// `running → done`.
pub fn mark_done(conn: &Connection, id: i64) -> Result<bool> {
    transition(conn, id, "'running'", "status = 'done', error = NULL")
}

/// `running → failed`; counts the attempt and keeps the human-readable reason.
pub fn mark_failed(conn: &Connection, id: i64, error: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE download_jobs SET status = 'failed', error = ?2, attempts = attempts + 1,
            updated_at = ?3
         WHERE id = ?1 AND status = 'running'",
        params![id, error, now_ms()],
    )?;
    Ok(n > 0)
}

/// `running → queued` for one job: YouTube throttled us, try again after the backoff.
pub fn requeue(conn: &Connection, id: i64) -> Result<bool> {
    transition(conn, id, "'running'", "status = 'queued'")
}

/// `running/queued → paused`. The worker must stop the running downloads itself
/// (their `.part` files are kept). Returns the number of paused jobs.
pub fn pause_all(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE download_jobs SET status = 'paused', updated_at = ?1
         WHERE status IN ('queued', 'running')",
        [now_ms()],
    )?)
}

/// `paused → queued`. Returns the number of resumed jobs.
pub fn resume_all(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE download_jobs SET status = 'queued', updated_at = ?1 WHERE status = 'paused'",
        [now_ms()],
    )?)
}

/// `failed → queued` for one job. Skipped if the track got another active job or was
/// downloaded meanwhile.
pub fn retry(conn: &Connection, id: i64) -> Result<bool> {
    let n = conn.execute(
        "UPDATE OR IGNORE download_jobs SET status = 'queued', error = NULL, updated_at = ?2
         WHERE id = ?1 AND status = 'failed'
           AND NOT EXISTS(SELECT 1 FROM tracks t WHERE t.id = track_id AND t.state = 'downloaded')",
        params![id, now_ms()],
    )?;
    Ok(n > 0)
}

/// `failed → queued` for every failed job (same rules as [`retry`]).
pub fn retry_all_failed(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE OR IGNORE download_jobs SET status = 'queued', error = NULL, updated_at = ?1
         WHERE status = 'failed'
           AND NOT EXISTS(SELECT 1 FROM tracks t WHERE t.id = track_id AND t.state = 'downloaded')",
        [now_ms()],
    )?)
}

/// Called at startup: jobs cut off by an exit go back to `queued` and resume from
/// their `.part` files.
pub fn requeue_running(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE download_jobs SET status = 'queued', updated_at = ?1 WHERE status = 'running'",
        [now_ms()],
    )?)
}

pub fn summary(conn: &Connection) -> Result<JobsSummary> {
    let mut s = JobsSummary::default();
    let mut stmt =
        conn.prepare_cached("SELECT status, count(*) FROM download_jobs GROUP BY status")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let n: u32 = r.get(1)?;
        match JobStatus::parse(&r.get::<_, String>(0)?) {
            Some(JobStatus::Queued) => s.queued = n,
            Some(JobStatus::Running) => s.running = n,
            Some(JobStatus::Paused) => s.paused = n,
            Some(JobStatus::Done) => s.done = n,
            Some(JobStatus::Failed) => s.failed = n,
            None => {}
        }
    }
    Ok(s)
}

/// Failed jobs, newest first, with track title and artists.
pub fn failed_list(conn: &Connection) -> Result<Vec<FailedJob>> {
    let mut stmt = conn.prepare_cached(
        "SELECT j.id, j.track_id, t.title,
                COALESCE((SELECT group_concat(name, ', ' ORDER BY position)
                          FROM track_artists WHERE track_id = j.track_id), ''),
                COALESCE(j.error, ''), j.attempts
         FROM download_jobs j JOIN tracks t ON t.id = j.track_id
         WHERE j.status = 'failed'
         ORDER BY j.updated_at DESC, j.id DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(FailedJob {
                id: r.get(0)?,
                track_id: TrackId(r.get(1)?),
                title: r.get(2)?,
                artists: r.get(3)?,
                error: r.get(4)?,
                attempts: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Deletes finished jobs. Returns how many were removed.
pub fn clear_done(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM download_jobs WHERE status = 'done'", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::{set_state, upsert_track};
    use mixtape_core::TrackState;

    fn tid(s: &str) -> TrackId {
        TrackId::from(s)
    }

    fn setup() -> (tempfile::TempDir, crate::Db) {
        let (d, db) = db();
        db.write(|t| {
            upsert_track(t, &meta("a", "Alpha", ("UC1", "X"), None))?;
            upsert_track(t, &meta("b", "Beta", ("UC2", "Y"), None))?;
            upsert_track(t, &meta("c", "Gamma", ("UC3", "Z"), None))?;
            Ok(())
        })
        .unwrap();
        (d, db)
    }

    fn status(db: &crate::Db, id: i64) -> JobStatus {
        db.read(|c| get(c, id)).unwrap().unwrap().status
    }

    #[test]
    fn one_active_job_per_track() {
        let (_d, db) = setup();
        let (first, second) = db
            .write(|t| {
                Ok((
                    enqueue(t, &tid("a"), "user")?,
                    enqueue(t, &tid("a"), "liked")?,
                ))
            })
            .unwrap();
        assert!(first.is_some());
        assert_eq!(second, None);
        let id = first.unwrap();
        // Still blocked while running and paused.
        db.write(move |t| {
            assert!(mark_running(t, id)?);
            assert_eq!(enqueue(t, &tid("a"), "user")?, None);
            pause_all(t)?;
            assert_eq!(enqueue(t, &tid("a"), "user")?, None);
            Ok(())
        })
        .unwrap();
        // A finished job does not block a new one.
        db.write(move |t| {
            resume_all(t)?;
            mark_running(t, id)?;
            assert!(mark_done(t, id)?);
            let again = enqueue(t, &tid("a"), "user")?;
            assert!(again.is_some_and(|n| n != id));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn downloaded_track_is_not_queued() {
        let (_d, db) = setup();
        let r = db
            .write(|t| {
                set_state(t, &tid("b"), TrackState::Downloaded)?;
                enqueue(t, &tid("b"), "user")
            })
            .unwrap();
        assert_eq!(r, None);
    }

    #[test]
    fn happy_path_and_progress() {
        let (_d, db) = setup();
        let id = db
            .write(|t| Ok(enqueue(t, &tid("a"), "url")?.unwrap()))
            .unwrap();
        let next = db.read(next_queued).unwrap().unwrap();
        assert_eq!(
            (next.id, next.status, next.source.as_str()),
            (id, JobStatus::Queued, "url")
        );
        // Progress is ignored before the job runs, and done needs running.
        db.write(move |t| {
            assert!(!progress(t, id, 5, Some(10))?);
            assert!(!mark_done(t, id)?);
            assert!(mark_running(t, id)?);
            assert!(!mark_running(t, id)?);
            assert!(progress(t, id, 5, Some(10))?);
            assert!(progress(t, id, 7, None)?);
            Ok(())
        })
        .unwrap();
        let j = db.read(|c| get(c, id)).unwrap().unwrap();
        assert_eq!((j.bytes_done, j.bytes_total), (7, Some(10)));
        assert_eq!(db.read(running).unwrap().len(), 1);
        assert_eq!(db.read(next_queued).unwrap(), None);
        db.write(move |t| mark_done(t, id)).unwrap();
        assert_eq!(status(&db, id), JobStatus::Done);
        assert_eq!(
            db.read(summary).unwrap(),
            JobsSummary {
                done: 1,
                ..Default::default()
            }
        );
        assert_eq!(db.write(|t| clear_done(t)).unwrap(), 1);
        assert_eq!(db.read(|c| get(c, id)).unwrap(), None);
    }

    #[test]
    fn next_queued_is_fifo() {
        let (_d, db) = setup();
        let (a, b) = db
            .write(|t| {
                Ok((
                    enqueue(t, &tid("a"), "user")?.unwrap(),
                    enqueue(t, &tid("b"), "user")?.unwrap(),
                ))
            })
            .unwrap();
        assert_eq!(db.read(next_queued).unwrap().unwrap().id, a);
        assert_eq!(db.read(|c| next_queued_tracks(c, 5)).unwrap(), [tid("a"), tid("b")]);
        db.write(move |t| mark_running(t, a)).unwrap();
        assert_eq!(db.read(next_queued).unwrap().unwrap().id, b);
        assert_eq!(db.read(|c| next_queued_tracks(c, 1)).unwrap(), [tid("b")]);
    }

    #[test]
    fn failure_retry_and_list() {
        let (_d, db) = setup();
        let (a, b) = db
            .write(|t| {
                let a = enqueue(t, &tid("a"), "user")?.unwrap();
                let b = enqueue(t, &tid("b"), "user")?.unwrap();
                // Only running jobs can fail.
                assert!(!mark_failed(t, a, "x")?);
                mark_running(t, a)?;
                mark_running(t, b)?;
                assert!(mark_failed(t, a, "Недоступно")?);
                assert!(mark_failed(t, b, "Нет формата")?);
                Ok((a, b))
            })
            .unwrap();
        let list = db.read(failed_list).unwrap();
        assert_eq!(list.len(), 2);
        let fa = list.iter().find(|f| f.id == a).unwrap();
        assert_eq!((fa.title.as_str(), fa.artists.as_str()), ("Alpha", "X"));
        assert_eq!((fa.error.as_str(), fa.attempts), ("Недоступно", 1));

        // Single retry: failed → queued, error cleared, attempts kept.
        assert!(db.write(move |t| retry(t, a)).unwrap());
        assert!(!db.write(move |t| retry(t, a)).unwrap());
        let j = db.read(|c| get(c, a)).unwrap().unwrap();
        assert_eq!(
            (j.status, j.error, j.attempts),
            (JobStatus::Queued, None, 1)
        );

        // Second failure counts the attempt again.
        db.write(move |t| {
            mark_running(t, a)?;
            mark_failed(t, a, "Снова")
        })
        .unwrap();
        assert_eq!(db.read(|c| get(c, a)).unwrap().unwrap().attempts, 2);

        assert_eq!(db.write(|t| retry_all_failed(t)).unwrap(), 2);
        assert_eq!(status(&db, b), JobStatus::Queued);
        assert!(db.read(failed_list).unwrap().is_empty());
    }

    #[test]
    fn enqueue_revives_failed_job() {
        let (_d, db) = setup();
        let id = db
            .write(|t| {
                let id = enqueue(t, &tid("c"), "user")?.unwrap();
                mark_running(t, id)?;
                mark_failed(t, id, "Ошибка")?;
                Ok(id)
            })
            .unwrap();
        let again = db.write(|t| enqueue(t, &tid("c"), "liked")).unwrap();
        assert_eq!(again, Some(id));
        let j = db.read(|c| get(c, id)).unwrap().unwrap();
        assert_eq!(
            (j.status, j.source.as_str(), j.error),
            (JobStatus::Queued, "liked", None)
        );
    }

    #[test]
    fn retry_skips_downloaded_track() {
        let (_d, db) = setup();
        let id = db
            .write(|t| {
                let id = enqueue(t, &tid("a"), "user")?.unwrap();
                mark_running(t, id)?;
                mark_failed(t, id, "Ошибка")?;
                set_state(t, &tid("a"), TrackState::Downloaded)?;
                Ok(id)
            })
            .unwrap();
        assert!(!db.write(move |t| retry(t, id)).unwrap());
        assert_eq!(db.write(|t| retry_all_failed(t)).unwrap(), 0);
    }

    #[test]
    fn pause_resume_and_restart() {
        let (_d, db) = setup();
        let (a, b, c) = db
            .write(|t| {
                let a = enqueue(t, &tid("a"), "user")?.unwrap();
                let b = enqueue(t, &tid("b"), "user")?.unwrap();
                let c = enqueue(t, &tid("c"), "user")?.unwrap();
                mark_running(t, a)?;
                mark_running(t, c)?;
                mark_failed(t, c, "Ошибка")?;
                Ok((a, b, c))
            })
            .unwrap();
        assert_eq!(db.write(|t| pause_all(t)).unwrap(), 2);
        assert_eq!(
            (status(&db, a), status(&db, b), status(&db, c)),
            (JobStatus::Paused, JobStatus::Paused, JobStatus::Failed)
        );
        assert_eq!(db.read(next_queued).unwrap(), None);
        // A paused job cannot start or report progress.
        assert!(!db.write(move |t| mark_running(t, a)).unwrap());
        assert!(!db.write(move |t| progress(t, a, 1, None)).unwrap());
        assert_eq!(
            db.read(summary).unwrap(),
            JobsSummary {
                paused: 2,
                failed: 1,
                ..Default::default()
            }
        );
        assert_eq!(db.write(|t| resume_all(t)).unwrap(), 2);
        assert_eq!(
            (status(&db, a), status(&db, b)),
            (JobStatus::Queued, JobStatus::Queued)
        );

        // Restart: running → queued, others untouched.
        db.write(move |t| mark_running(t, a)).unwrap();
        assert_eq!(db.write(|t| requeue_running(t)).unwrap(), 1);
        assert_eq!(
            (status(&db, a), status(&db, c)),
            (JobStatus::Queued, JobStatus::Failed)
        );
        let s = db.read(summary).unwrap();
        assert_eq!(
            s,
            JobsSummary {
                queued: 2,
                failed: 1,
                ..Default::default()
            }
        );
        assert_eq!(s.pending(), 2);
    }

    #[test]
    fn status_roundtrip() {
        for s in [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Paused,
            JobStatus::Done,
            JobStatus::Failed,
        ] {
            assert_eq!(JobStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(JobStatus::parse("nope"), None);
    }
}
