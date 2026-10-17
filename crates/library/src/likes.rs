//! Local likes and the `like_outbox` that carries them to YouTube Music.
//!
//! One row per track: a new action replaces the old one, so like-then-unlike before a
//! send leaves only the unlike. When the last action matches what YouTube already has,
//! the row is still sent — setting a like to its current value is harmless.

use crate::Result;
use crate::repo::{now_ms, set_liked};
use mixtape_core::TrackId;
use rusqlite::{Connection, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLike {
    pub id: i64,
    pub track_id: TrackId,
    pub liked: bool,
    pub attempts: u32,
}

/// Sets the local like and queues it for YouTube Music. The queued row gets a new id
/// above every existing one, so a send still in flight for the old action cannot mark
/// the new one as sent.
pub fn queue_like(conn: &Connection, id: &TrackId, liked: bool) -> Result<()> {
    set_liked(conn, id, liked)?;
    let next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(id), 0) + 1 FROM like_outbox",
        [],
        |r| r.get(0),
    )?;
    conn.execute("DELETE FROM like_outbox WHERE track_id = ?1", [id.as_str()])?;
    conn.execute(
        "INSERT INTO like_outbox(id, track_id, action, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            next,
            id.as_str(),
            if liked { "like" } else { "unlike" },
            now_ms()
        ],
    )?;
    Ok(())
}

/// Queued actions, oldest first.
pub fn pending_likes(conn: &Connection, limit: usize) -> Result<Vec<PendingLike>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, track_id, action, attempts FROM like_outbox ORDER BY id LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(PendingLike {
            id: r.get(0)?,
            track_id: TrackId(r.get(1)?),
            liked: r.get::<_, String>(2)? == "like",
            attempts: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The action reached YouTube Music (or will never be accepted): drops it.
pub fn like_sent(conn: &Connection, outbox_id: i64) -> Result<()> {
    conn.execute("DELETE FROM like_outbox WHERE id = ?1", [outbox_id])?;
    Ok(())
}

/// A send failed; the row stays for a later retry.
pub fn like_failed(conn: &Connection, outbox_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE like_outbox SET attempts = attempts + 1 WHERE id = ?1",
        [outbox_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::{track, upsert_track};
    use crate::ytm::apply_remote_likes;

    #[test]
    fn last_action_wins_and_in_flight_send_keeps_it() {
        let (_dir, db) = db();
        let a = TrackId::from("a");
        let wa = a.clone();
        let (first, after) = db
            .write(move |t| {
                upsert_track(t, &meta("a", "A", ("UC1", "X"), None))?;
                upsert_track(t, &meta("b", "B", ("UC1", "X"), None))?;
                queue_like(t, &wa, true)?;
                queue_like(t, &TrackId::from("b"), true)?;
                let first = pending_likes(t, 10)?;
                // Unlike while the like is being sent: the late `like_sent` must not drop it.
                queue_like(t, &wa, false)?;
                like_sent(t, first[0].id)?;
                Ok((first, pending_likes(t, 10)?))
            })
            .unwrap();
        assert_eq!(first.len(), 2);
        assert!(first[0].liked && first[0].track_id == a);
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].track_id, TrackId::from("b"));
        assert_eq!(
            (after[1].track_id.clone(), after[1].liked),
            (a.clone(), false)
        );
        assert!(!db.read(|c| track(c, &a)).unwrap().unwrap().liked);
    }

    #[test]
    fn failures_count_and_outbox_beats_remote_sync() {
        let (_dir, db) = db();
        let a = TrackId::from("a");
        let wa = a.clone();
        let pending = db
            .write(move |t| {
                upsert_track(t, &meta("a", "A", ("UC1", "X"), None))?;
                queue_like(t, &wa, true)?;
                let id = pending_likes(t, 1)?[0].id;
                like_failed(t, id)?;
                like_failed(t, id)?;
                // YouTube does not know about the like yet; the local value stays.
                apply_remote_likes(t, &[])?;
                pending_likes(t, 10)
            })
            .unwrap();
        assert_eq!(pending[0].attempts, 2);
        assert!(db.read(|c| track(c, &a)).unwrap().unwrap().liked);
    }
}
