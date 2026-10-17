//! The `scrobble_outbox`: Last.fm scrobbles waiting to be sent.
//!
//! A play that passed the scrobble rules is queued here first and sent from here, so a
//! scrobble made offline or during a Last.fm outage goes out later. Rows are sent in
//! batches oldest first; a failed batch counts an attempt on each of its rows.

use crate::Result;
use rusqlite::{Connection, OptionalExtension, params};

/// One play to scrobble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrobble {
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub duration_s: Option<u32>,
    /// When the play started, Unix seconds.
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingScrobble {
    pub id: i64,
    pub scrobble: Scrobble,
    pub attempts: u32,
}

/// Sends tried before a row is given up.
pub const MAX_ATTEMPTS: u32 = 20;

/// Last.fm ignores scrobbles older than two weeks; such rows are dropped unsent.
pub const MAX_AGE_S: i64 = 14 * 24 * 3600;

/// Queues a play. A second row with the same start time is ignored. Returns whether a
/// row was added.
pub fn queue(conn: &Connection, s: &Scrobble) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO scrobble_outbox(artist, track, album, duration_s, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![s.artist, s.track, s.album, s.duration_s, s.started_at],
    )?;
    Ok(n > 0)
}

/// Queued plays, oldest first.
pub fn pending(conn: &Connection, limit: usize) -> Result<Vec<PendingScrobble>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, artist, track, album, duration_s, started_at, attempts
         FROM scrobble_outbox ORDER BY started_at, id LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(PendingScrobble {
            id: r.get(0)?,
            scrobble: Scrobble {
                artist: r.get(1)?,
                track: r.get(2)?,
                album: r.get(3)?,
                duration_s: r.get(4)?,
                started_at: r.get(5)?,
            },
            attempts: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The rows reached Last.fm (accepted or ignored there): drops them.
pub fn sent(conn: &Connection, ids: &[i64]) -> Result<()> {
    let mut stmt = conn.prepare_cached("DELETE FROM scrobble_outbox WHERE id = ?1")?;
    for id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

/// A send failed: each row counts an attempt and is dropped once it has failed
/// [`MAX_ATTEMPTS`] times. Returns how many were dropped.
pub fn failed(conn: &Connection, ids: &[i64]) -> Result<usize> {
    let mut bump =
        conn.prepare_cached("UPDATE scrobble_outbox SET attempts = attempts + 1 WHERE id = ?1")?;
    let mut drop =
        conn.prepare_cached("DELETE FROM scrobble_outbox WHERE id = ?1 AND attempts >= ?2")?;
    let mut dropped = 0;
    for id in ids {
        bump.execute([id])?;
        dropped += drop.execute(params![id, MAX_ATTEMPTS])?;
    }
    Ok(dropped)
}

/// Drops rows Last.fm would ignore for their age (`now_s` in Unix seconds). Returns how
/// many.
pub fn drop_expired(conn: &Connection, now_s: i64) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM scrobble_outbox WHERE started_at < ?1",
        [now_s - MAX_AGE_S],
    )?)
}

/// Failed attempts of the oldest queued row (the one a flush tries first), or `None`
/// when nothing is queued.
pub fn oldest_attempts(conn: &Connection) -> Result<Option<u32>> {
    Ok(conn
        .query_row(
            "SELECT attempts FROM scrobble_outbox ORDER BY started_at, id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

/// Removes every queued row (the Last.fm account was disconnected).
pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM scrobble_outbox", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::db;

    fn play(at: i64) -> Scrobble {
        Scrobble {
            artist: "a-ha".into(),
            track: format!("Song {at}"),
            album: Some("Hunting High and Low".into()),
            duration_s: Some(225),
            started_at: at,
        }
    }

    #[test]
    fn queues_once_per_start_and_sends_oldest_first() {
        let (_d, db) = db();
        let (added, rows) = db
            .write(|t| {
                let added = [
                    queue(t, &play(300))?,
                    queue(t, &play(100))?,
                    queue(t, &play(100))?,
                    queue(t, &play(200))?,
                ];
                Ok((added, pending(t, 2)?))
            })
            .unwrap();
        assert_eq!(added, [true, true, false, true]);
        assert_eq!(
            rows.iter()
                .map(|r| r.scrobble.started_at)
                .collect::<Vec<_>>(),
            [100, 200]
        );
        assert_eq!(rows[0].scrobble, play(100));
        let left = db
            .write(move |t| {
                sent(t, &rows.iter().map(|r| r.id).collect::<Vec<_>>())?;
                pending(t, 50)
            })
            .unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].scrobble.started_at, 300);
    }

    #[test]
    fn failures_count_and_give_up_after_the_cap() {
        let (_d, db) = db();
        let (dropped_at, left) = db
            .write(|t| {
                assert_eq!(oldest_attempts(t)?, None);
                queue(t, &play(100))?;
                queue(t, &play(200))?;
                assert_eq!(oldest_attempts(t)?, Some(0));
                let ids: Vec<i64> = pending(t, 50)?.iter().map(|r| r.id).collect();
                failed(t, &ids)?;
                assert_eq!(oldest_attempts(t)?, Some(1));
                let mut dropped_at = None;
                for n in 2..=MAX_ATTEMPTS {
                    if failed(t, &ids[..1])? > 0 {
                        dropped_at = Some(n);
                        break;
                    }
                }
                Ok((dropped_at, pending(t, 50)?))
            })
            .unwrap();
        assert_eq!(dropped_at, Some(MAX_ATTEMPTS));
        assert_eq!(left.len(), 1);
        assert_eq!((left[0].scrobble.started_at, left[0].attempts), (200, 1));
    }

    #[test]
    fn expired_rows_and_clear() {
        let (_d, db) = db();
        let now = 10 * MAX_AGE_S;
        let left = db
            .write(move |t| {
                queue(t, &play(now - MAX_AGE_S - 1))?;
                queue(t, &play(now - 60))?;
                assert_eq!(drop_expired(t, now)?, 1);
                pending(t, 50)
            })
            .unwrap();
        assert_eq!(left.len(), 1);
        db.write(|t| clear(t)).unwrap();
        assert_eq!(db.read(oldest_attempts).unwrap(), None);
    }
}
