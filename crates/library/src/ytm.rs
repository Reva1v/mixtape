//! Library writes specific to YouTube Music sync: mirrored playlists, remote likes and
//! per-source sync status.

use crate::normalize::normalize;
use crate::repo::now_ms;
use crate::{Result, playlists};
use mixtape_core::{AlbumId, PlaylistId, TrackId};
use rusqlite::{Connection, OptionalExtension, params};

/// Creates or refreshes a `ytm` playlist and replaces its items with `tracks`.
/// A playlist the user already turned into a local copy keeps its own id, so the copy
/// is never touched.
pub fn mirror_playlist(
    conn: &Connection,
    id: &PlaylistId,
    title: &str,
    cover_key: Option<&str>,
    tracks: &[TrackId],
) -> Result<()> {
    conn.execute(
        "INSERT INTO playlists(id, title, origin, cover_key, synced_at, search_text)
         VALUES (?1, ?2, 'ytm', ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET title = excluded.title,
            cover_key = COALESCE(excluded.cover_key, playlists.cover_key),
            synced_at = excluded.synced_at, search_text = excluded.search_text",
        params![id.as_str(), title, cover_key, now_ms(), normalize(title)],
    )?;
    playlists::rewrite(conn, id, tracks)
}

/// Makes local likes match YouTube Music. `liked` is newest first, as YouTube lists it;
/// `liked_at` keeps that order. Tracks with a like change still waiting in the outbox
/// keep the local value — it is newer than what YouTube knows.
pub fn apply_remote_likes(conn: &Connection, liked: &[TrackId]) -> Result<()> {
    let now = now_ms();
    conn.execute(
        "UPDATE tracks SET liked = 0, liked_at = NULL
         WHERE liked = 1 AND id NOT IN (SELECT track_id FROM like_outbox)",
        [],
    )?;
    let mut stmt = conn.prepare_cached(
        "UPDATE tracks SET liked = 1, liked_at = COALESCE(liked_at, ?2)
         WHERE id = ?1 AND id NOT IN (SELECT track_id FROM like_outbox)",
    )?;
    for (i, id) in liked.iter().enumerate() {
        stmt.execute(params![id.as_str(), now - i as i64])?;
    }
    Ok(())
}

pub fn album_exists(conn: &Connection, id: &AlbumId) -> Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM albums WHERE id = ?1", [id.as_str()], |_| Ok(())).optional()?.is_some())
}

/// Marks albums the user saved in YouTube Music.
pub fn set_album_in_library(conn: &Connection, id: &AlbumId) -> Result<()> {
    conn.execute("UPDATE albums SET in_library = 1 WHERE id = ?1", [id.as_str()])?;
    Ok(())
}

/// Ids among `ids` whose audio is already in the library.
pub fn downloaded(conn: &Connection, ids: &[TrackId]) -> Result<std::collections::HashSet<TrackId>> {
    let mut stmt = conn.prepare_cached("SELECT 1 FROM tracks WHERE id = ?1 AND state = 'downloaded'")?;
    let mut out = std::collections::HashSet::new();
    for id in ids {
        if stmt.exists([id.as_str()])? {
            out.insert(id.clone());
        }
    }
    Ok(out)
}

/// Records the outcome of syncing one source (`liked`, `playlists`, `albums`, `artists`, `url:<id>`).
pub fn source_synced(conn: &Connection, kind: &str, error: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_sources(kind, enabled, last_synced_at, last_error) VALUES (?1, 1, ?2, ?3)
         ON CONFLICT(kind) DO UPDATE SET
            last_synced_at = CASE WHEN ?3 IS NULL THEN ?2 ELSE sync_sources.last_synced_at END,
            last_error = ?3",
        params![kind, now_ms(), error],
    )?;
    Ok(())
}

pub fn last_synced_at(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row("SELECT MAX(last_synced_at) FROM sync_sources", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{self, tests as test_util};

    #[test]
    fn mirror_and_likes() {
        let (_dir, db) = test_util::db();
        let a = TrackId::new("aaaaaaaaaaa");
        let b = TrackId::new("bbbbbbbbbbb");
        let (wa, wb) = (a.clone(), b.clone());
        db.write(move |t| {
            repo::upsert_track(t, &test_util::meta("aaaaaaaaaaa", "A", ("UCx", "X"), None))?;
            repo::upsert_track(t, &test_util::meta("bbbbbbbbbbb", "B", ("UCx", "X"), None))?;
            repo::set_liked(t, &wa, true)?;
            mirror_playlist(t, &PlaylistId::new("VLPL1"), "Mix", None, &[wb.clone(), wa.clone()])?;
            apply_remote_likes(t, std::slice::from_ref(&wb))
        })
        .unwrap();
        let (ra, rb) = (a.clone(), b.clone());
        let (liked_a, liked_b, items) = db
            .read(move |c| {
                Ok((
                    repo::track(c, &ra)?.unwrap().liked,
                    repo::track(c, &rb)?.unwrap().liked,
                    repo::playlist_track_ids(c, &PlaylistId::new("VLPL1"))?,
                ))
            })
            .unwrap();
        assert!(!liked_a && liked_b);
        assert_eq!(items, vec![b, a]);
    }
}
