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

/// Gives artist credits that came without an id (local imports, unlinked names) the id
/// of a known artist with exactly that name, or of a new local artist (`localart…`), so
/// every name links to an artist page. Returns the number of credits linked.
pub fn link_artists_by_name(conn: &Connection) -> Result<usize> {
    let tables = ["track_artists", "album_artists"];
    for table in tables {
        let names: Vec<String> = conn
            .prepare(&format!(
                "SELECT DISTINCT x.name FROM {table} x
                 WHERE x.artist_id IS NULL AND x.name <> ''
                   AND NOT EXISTS (SELECT 1 FROM artists a WHERE a.name = x.name)"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for name in names {
            conn.execute(
                "INSERT OR IGNORE INTO artists(id, name, search_text) VALUES ('localart' || lower(hex(?1)), ?1, ?2)",
                params![name, normalize(&name)],
            )?;
        }
    }
    let mut n = 0;
    for table in tables {
        n += conn.execute(
            &format!(
                "UPDATE {table} SET artist_id = (SELECT a.id FROM artists a WHERE a.name = {table}.name
                    ORDER BY a.id LIKE 'localart%', a.id LIMIT 1)
                 WHERE artist_id IS NULL AND name <> ''"
            ),
            [],
        )?;
    }
    Ok(n)
}

/// Folds tracks from a local folder import (`local…` ids) into the YouTube Music track
/// they duplicate — same title, length within 3 s, already downloaded. Plays, likes,
/// playlist slots and the saved queue move to the YouTube track; the local row, and
/// albums or artists left without tracks, are removed. Audio files are not touched.
/// Returns the number of tracks merged.
pub fn merge_local_twins(conn: &Connection) -> Result<usize> {
    let pairs: Vec<(String, String)> = conn
        .prepare(
            "SELECT l.id, (SELECT y.id FROM tracks y
                 WHERE y.title = l.title AND y.id NOT LIKE 'local%' AND y.state = 'downloaded'
                   AND abs(y.duration_ms - l.duration_ms) < 3000 LIMIT 1)
             FROM tracks l WHERE l.id LIKE 'local%'",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
        .filter_map(|r| match r {
            Ok((l, Some(y))) => Some(Ok((l, y))),
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        })
        .collect::<rusqlite::Result<_>>()?;
    if pairs.is_empty() {
        return Ok(0);
    }
    let mut snapshot: Option<String> =
        conn.query_row("SELECT value FROM settings WHERE key = 'queue_snapshot'", [], |r| r.get(0)).optional()?;
    for (local, yt) in &pairs {
        conn.execute(
            "INSERT INTO play_stats(track_id, play_count, full_play_count, last_played_at)
             SELECT ?2, play_count, full_play_count, last_played_at FROM play_stats WHERE track_id = ?1
             ON CONFLICT(track_id) DO UPDATE SET
                play_count = play_stats.play_count + excluded.play_count,
                full_play_count = play_stats.full_play_count + excluded.full_play_count,
                last_played_at = MAX(COALESCE(play_stats.last_played_at, 0), COALESCE(excluded.last_played_at, 0))",
            params![local, yt],
        )?;
        conn.execute(
            "UPDATE tracks SET liked = 1, liked_at = COALESCE(liked_at, (SELECT liked_at FROM tracks WHERE id = ?1))
             WHERE id = ?2 AND (SELECT liked FROM tracks WHERE id = ?1) = 1",
            params![local, yt],
        )?;
        conn.execute("UPDATE playlist_items SET track_id = ?2 WHERE track_id = ?1", params![local, yt])?;
        if let Some(s) = &mut snapshot {
            *s = s.replace(&format!("\"{local}\""), &format!("\"{yt}\""));
        }
        conn.execute("DELETE FROM tracks WHERE id = ?1", [local])?;
    }
    if let Some(s) = snapshot {
        conn.execute("UPDATE settings SET value = ?1 WHERE key = 'queue_snapshot'", [s])?;
    }
    conn.execute(
        "DELETE FROM albums WHERE id LIKE 'localalb%' AND NOT EXISTS (SELECT 1 FROM tracks WHERE album_id = albums.id)",
        [],
    )?;
    conn.execute(
        "DELETE FROM artists WHERE id LIKE 'localart%'
           AND NOT EXISTS (SELECT 1 FROM track_artists WHERE artist_id = artists.id)
           AND NOT EXISTS (SELECT 1 FROM album_artists WHERE artist_id = artists.id)",
        [],
    )?;
    Ok(pairs.len())
}

/// Artists credited on any track or album that have no picture yet.
pub fn artists_without_image(conn: &Connection) -> Result<Vec<mixtape_core::ArtistId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM artists WHERE image_key IS NULL AND id LIKE 'UC%'
           AND (EXISTS (SELECT 1 FROM track_artists WHERE artist_id = artists.id)
             OR EXISTS (SELECT 1 FROM album_artists WHERE artist_id = artists.id))",
    )?;
    let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids.into_iter().map(mixtape_core::ArtistId).collect())
}

pub fn set_artist_image(conn: &Connection, id: &mixtape_core::ArtistId, key: &str, palette: &[String]) -> Result<()> {
    let palette = (!palette.is_empty()).then(|| serde_json::to_string(palette)).transpose()?;
    conn.execute(
        "UPDATE artists SET image_key = ?2, palette = COALESCE(?3, palette) WHERE id = ?1",
        params![id.as_str(), key, palette],
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
    fn unlinked_credits_get_known_ids() {
        let (_dir, db) = test_util::db();
        db.write(|t| {
            repo::upsert_track(t, &test_util::meta("aaaaaaaaaaa", "A", ("UCx", "X"), None))?;
            let mut local = test_util::meta("bbbbbbbbbbb", "B", ("UCx", "X"), None);
            local.artists[0].id = None;
            repo::upsert_track(t, &local)?;
            assert_eq!(link_artists_by_name(t)?, 1);
            Ok(())
        })
        .unwrap();
        let linked: i64 = db
            .read(|c| Ok(c.query_row("SELECT count(*) FROM track_artists WHERE artist_id = 'UCx'", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(linked, 2);
    }

    #[test]
    fn local_import_folds_into_its_twin() {
        let (_dir, db) = test_util::db();
        let merged = db
            .write(|t| {
                repo::upsert_track(t, &test_util::meta("localabc", "Song", ("UCx", "X"), None))?;
                repo::upsert_track(t, &test_util::meta("yyyyyyyyyyy", "Song", ("UCx", "X"), None))?;
                repo::set_state(t, &TrackId::new("yyyyyyyyyyy"), mixtape_core::TrackState::Downloaded)?;
                repo::record_play(t, &TrackId::new("localabc"), true)?;
                repo::set_liked(t, &TrackId::new("localabc"), true)?;
                t.execute("INSERT INTO settings(key, value) VALUES ('queue_snapshot', '{\"ids\":[\"localabc\"]}')", [])?;
                merge_local_twins(t)
            })
            .unwrap();
        assert_eq!(merged, 1);
        let (gone, liked, plays, snap) = db
            .read(|c| {
                Ok((
                    repo::track(c, &TrackId::new("localabc"))?.is_none(),
                    repo::track(c, &TrackId::new("yyyyyyyyyyy"))?.unwrap().liked,
                    c.query_row("SELECT play_count FROM play_stats WHERE track_id = 'yyyyyyyyyyy'", [], |r| r.get::<_, i64>(0))?,
                    c.query_row("SELECT value FROM settings WHERE key = 'queue_snapshot'", [], |r| r.get::<_, String>(0))?,
                ))
            })
            .unwrap();
        assert!(gone && liked);
        assert_eq!(plays, 1);
        assert!(snap.contains("yyyyyyyyyyy"));
    }

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
