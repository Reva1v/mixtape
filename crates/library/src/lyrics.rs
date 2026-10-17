//! Lyrics cache in `lyrics_cache` (data-model.md): the provider's raw text per track.
//!
//! `body` is LRC when `synced`, plain text otherwise; an empty body marks an
//! instrumental and `NULL` means the provider had nothing ("not found").

use crate::Result;
use crate::repo::now_ms;
use mixtape_core::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

/// One cached lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedLyrics {
    pub synced: bool,
    /// `None` = not found.
    pub body: Option<String>,
    /// Unix ms of the lookup.
    pub fetched_at: i64,
}

pub fn get(conn: &Connection, id: &TrackId) -> Result<Option<CachedLyrics>> {
    Ok(conn
        .query_row(
            "SELECT synced, body, fetched_at FROM lyrics_cache WHERE track_id = ?1",
            [id.as_str()],
            |r| {
                Ok(CachedLyrics {
                    synced: r.get(0)?,
                    body: r.get(1)?,
                    fetched_at: r.get(2)?,
                })
            },
        )
        .optional()?)
}

/// Stores a lookup result (`body = None` for "not found"), replacing the previous one.
pub fn put(conn: &Connection, id: &TrackId, synced: bool, body: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO lyrics_cache (track_id, synced, body, fetched_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(track_id) DO UPDATE SET
             synced = excluded.synced, body = excluded.body, fetched_at = excluded.fetched_at",
        params![id.as_str(), synced, body, now_ms()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::db;

    #[test]
    fn put_get_and_replace() {
        let (_d, db) = db();
        let id = TrackId::from("t1");
        assert_eq!(db.read(|c| get(c, &id)).unwrap(), None);
        db.write(|t| put(t, &TrackId::from("t1"), false, None))
            .unwrap();
        let miss = db.read(|c| get(c, &id)).unwrap().unwrap();
        assert!(miss.body.is_none() && !miss.synced && miss.fetched_at > 0);
        db.write(|t| put(t, &TrackId::from("t1"), true, Some("[00:01.00]Hi")))
            .unwrap();
        let hit = db.read(|c| get(c, &id)).unwrap().unwrap();
        assert!(hit.synced);
        assert_eq!(hit.body.as_deref(), Some("[00:01.00]Hi"));
    }
}
