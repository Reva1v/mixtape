//! Playlist editing. Playlists synced from YouTube Music are read-only mirrors: the
//! first edit turns them into a local copy, so the next sync cannot overwrite the
//! user's changes.

use crate::normalize::normalize;
use crate::repo::now_ms;
use crate::{Error, Result};
use mixtape_core::{PlaylistId, TrackId};
use rusqlite::{Connection, OptionalExtension, params};

fn new_local_id() -> PlaylistId {
    PlaylistId(format!("{}{:x}{:04x}", PlaylistId::LOCAL_PREFIX, now_ms(), fastrand::u16(..)))
}

pub fn create(conn: &Connection, title: &str) -> Result<PlaylistId> {
    let id = new_local_id();
    conn.execute(
        "INSERT INTO playlists(id, title, origin, search_text) VALUES (?1, ?2, 'local', ?3)",
        params![id.as_str(), title, normalize(title)],
    )?;
    Ok(id)
}

pub fn rename(conn: &Connection, id: &PlaylistId, title: &str) -> Result<PlaylistId> {
    let id = editable(conn, id)?;
    conn.execute(
        "UPDATE playlists SET title = ?2, search_text = ?3 WHERE id = ?1",
        params![id.as_str(), title, normalize(title)],
    )?;
    Ok(id)
}

pub fn delete(conn: &Connection, id: &PlaylistId) -> Result<()> {
    conn.execute("DELETE FROM playlists WHERE id = ?1", [id.as_str()])?;
    Ok(())
}

/// Appends tracks; returns the id actually edited (a copy if `id` was a YTM mirror).
pub fn add(conn: &Connection, id: &PlaylistId, tracks: &[TrackId]) -> Result<PlaylistId> {
    let id = editable(conn, id)?;
    let start: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_items WHERE playlist_id = ?1",
        [id.as_str()],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare_cached(
        "INSERT INTO playlist_items(playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
    )?;
    for (i, t) in tracks.iter().enumerate() {
        stmt.execute(params![id.as_str(), start + i as i64, t.as_str()])?;
    }
    Ok(id)
}

/// Removes the item at `position` and closes the gap.
pub fn remove(conn: &Connection, id: &PlaylistId, position: u32) -> Result<PlaylistId> {
    let id = editable(conn, id)?;
    let mut order = items(conn, &id)?;
    if (position as usize) < order.len() {
        order.remove(position as usize);
    }
    rewrite(conn, &id, &order)?;
    Ok(id)
}

pub fn move_item(conn: &Connection, id: &PlaylistId, from: u32, to: u32) -> Result<PlaylistId> {
    let id = editable(conn, id)?;
    let mut order = items(conn, &id)?;
    if (from as usize) < order.len() {
        let t = order.remove(from as usize);
        order.insert((to as usize).min(order.len()), t);
    }
    rewrite(conn, &id, &order)?;
    Ok(id)
}

/// Replaces the content of a playlist (used by sync for YTM mirrors).
pub fn rewrite(conn: &Connection, id: &PlaylistId, tracks: &[TrackId]) -> Result<()> {
    conn.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id.as_str()])?;
    let mut stmt = conn.prepare_cached(
        "INSERT INTO playlist_items(playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
    )?;
    for (i, t) in tracks.iter().enumerate() {
        stmt.execute(params![id.as_str(), i as i64, t.as_str()])?;
    }
    Ok(())
}

fn items(conn: &Connection, id: &PlaylistId) -> Result<Vec<TrackId>> {
    crate::repo::playlist_track_ids(conn, id)
}

/// Returns `id` for a local playlist, or a fresh local copy of a YTM mirror.
fn editable(conn: &Connection, id: &PlaylistId) -> Result<PlaylistId> {
    let row: Option<(String, String)> = conn
        .query_row("SELECT origin, title FROM playlists WHERE id = ?1", [id.as_str()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let (origin, title) = row.ok_or_else(|| Error::Other(mixtape_core::tr("Playlist not found", "Плейлист не знайдено", "Плейлист не найден").into()))?;
    if origin == "local" {
        return Ok(id.clone());
    }
    let copy = create(conn, &title)?;
    let tracks = items(conn, id)?;
    rewrite(conn, &copy, &tracks)?;
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::{playlist_track_ids, upsert_track};

    fn names(v: Vec<TrackId>) -> Vec<String> {
        v.into_iter().map(|t| t.0).collect()
    }

    #[test]
    fn edit_local_and_copy_ytm() {
        let (_d, db) = db();
        let (local, copy, ytm) = db
            .write(|t| {
                for id in ["a", "b", "c"] {
                    upsert_track(t, &meta(id, id, ("UC1", "X"), None))?;
                }
                let local = create(t, "Мой")?;
                let tracks = [TrackId::from("a"), TrackId::from("b"), TrackId::from("c")];
                add(t, &local, &tracks)?;
                move_item(t, &local, 2, 0)?;
                remove(t, &local, 1)?;

                let ytm = PlaylistId::from("VLPLx");
                t.execute("INSERT INTO playlists(id, title, origin) VALUES ('VLPLx', 'YT', 'ytm')", [])?;
                rewrite(t, &ytm, &tracks)?;
                let copy = add(t, &ytm, &[TrackId::from("a")])?;
                Ok((local, copy, ytm))
            })
            .unwrap();
        assert!(local.is_local());
        assert_eq!(names(db.read(|c| playlist_track_ids(c, &local)).unwrap()), ["c", "b"]);
        assert!(copy.is_local() && copy != ytm);
        assert_eq!(names(db.read(|c| playlist_track_ids(c, &ytm)).unwrap()), ["a", "b", "c"]);
        assert_eq!(names(db.read(|c| playlist_track_ids(c, &copy)).unwrap()), ["a", "b", "c", "a"]);
    }
}
