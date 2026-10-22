//! Opening connections and applying migrations.

use crate::Result;
use rusqlite::{Connection, OpenFlags};
use rusqlite_migration::{M, Migrations};
use std::path::Path;
use std::time::Duration;

const MIGRATIONS_SLICE: &[M<'static>] = &[
    M::up(include_str!("../migrations/001_init.sql")),
    M::up(include_str!("../migrations/002_artist_palette.sql")),
    M::up(include_str!("../migrations/003_artist_image_checked.sql")),
    M::up(include_str!("../migrations/004_play_events.sql")),
    M::up(include_str!("../migrations/005_scrobble_outbox.sql")),
    M::up(include_str!("../migrations/006_track_loudness.sql")),
    M::up(include_str!("../migrations/007_track_analysis.sql")),
    M::up(include_str!("../migrations/008_media_containers.sql")),
    M::up(include_str!("../migrations/009_play_events_device.sql")),
];

/// Creates `settings.device_id` (a random UUID v4, as JSON) when it is missing. Migration 009
/// makes it the same way; this covers a row deleted by hand.
const ENSURE_DEVICE_ID: &str = "INSERT OR IGNORE INTO settings(key, value) VALUES ('device_id', '\"' ||
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))), 2) || '-' || substr('89ab', 1 + (abs(random()) % 4), 1) ||
    substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))) || '\"')";

fn migrations() -> Migrations<'static> {
    Migrations::from_slice(MIGRATIONS_SLICE)
}

/// Opens the writer connection and brings the schema up to date.
pub fn open_writer(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // After a checkpoint the WAL file shrinks back to 1 MB instead of staying at its
    // high-water mark (a sync can grow it to several MB).
    conn.pragma_update(None, "journal_size_limit", 1_048_576)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -16_000)?; // ~16 MB
    conn.busy_timeout(Duration::from_secs(5))?;
    migrations().to_latest(&mut conn)?;
    conn.execute(ENSURE_DEVICE_ID, [])?;
    Ok(conn)
}

/// Opens a read-only connection. Call after [`open_writer`] has created the schema.
pub fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "cache_size", -8_000)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(conn: &Connection, q: &str) -> i64 {
        conn.query_row("SELECT count(*) FROM tracks_fts WHERE tracks_fts MATCH ?1", [q], |r| {
            r.get(0)
        })
        .unwrap()
    }

    #[test]
    fn migrations_are_valid() {
        migrations().validate().unwrap();
    }

    #[test]
    fn v2_database_gains_image_checked_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            Migrations::from_slice(&MIGRATIONS_SLICE[..2]).to_latest(&mut conn).unwrap();
            conn.execute("INSERT INTO artists(id, name) VALUES ('UCa', 'A')", []).unwrap();
        }
        let conn = open_writer(&path).unwrap();
        let checked: Option<i64> =
            conn.query_row("SELECT image_checked_at FROM artists WHERE id = 'UCa'", [], |r| r.get(0)).unwrap();
        assert_eq!(checked, None);
        let limit: i64 = conn.query_row("PRAGMA journal_size_limit", [], |r| r.get(0)).unwrap();
        assert_eq!(limit, 1_048_576);
    }

    #[test]
    fn v7_media_files_keep_rows_and_accept_mp3() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            Migrations::from_slice(&MIGRATIONS_SLICE[..7]).to_latest(&mut conn).unwrap();
            conn.execute("INSERT INTO tracks(id, title) VALUES ('a', 'A'), ('b', 'B')", []).unwrap();
            conn.execute(
                "INSERT INTO media_files(track_id, path, container, size_bytes, lufs, last_access, location)
                 VALUES ('a', 'library/a.webm', 'webm', 10, -9.5, 1, 'library')",
                [],
            )
            .unwrap();
            assert!(conn
                .execute("INSERT INTO media_files(track_id, path, container, size_bytes, last_access, location) VALUES ('b', 'x', 'mp3', 1, 1, 'library')", [])
                .is_err());
        }
        let conn = open_writer(&path).unwrap();
        let row: (String, String, f64) = conn
            .query_row("SELECT path, container, lufs FROM media_files WHERE track_id = 'a'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(row, ("library/a.webm".into(), "webm".into(), -9.5));
        conn.execute("INSERT INTO media_files(track_id, path, container, size_bytes, last_access, location) VALUES ('b', 'library/b.mp3', 'mp3', 1, 1, 'library')", [])
            .unwrap();
        assert!(conn
            .execute("UPDATE media_files SET container = 'flac' WHERE track_id = 'b'", [])
            .is_err());
        // Still cascades with its track.
        conn.execute("DELETE FROM tracks WHERE id = 'b'", []).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM media_files", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn v8_play_events_get_this_device_and_seq() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            Migrations::from_slice(&MIGRATIONS_SLICE[..8]).to_latest(&mut conn).unwrap();
            conn.execute("INSERT INTO tracks(id, title) VALUES ('a', 'A')", []).unwrap();
            conn.execute(
                "INSERT INTO play_events(id, track_id, started_at, listened_ms) VALUES (3, 'a', 10, 1), (7, 'a', 20, 2)",
                [],
            )
            .unwrap();
        }
        let conn = open_writer(&path).unwrap();
        let device = crate::settings::device_id(&conn).unwrap();
        assert_eq!(device.len(), 36);
        assert_eq!(&device[14..15], "4", "UUID v4: {device}");
        assert!("89ab".contains(&device[19..20]), "UUID variant: {device}");
        let rows: Vec<(i64, String, i64)> = conn
            .prepare("SELECT id, device, seq FROM play_events ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, [(3, device.clone(), 3), (7, device.clone(), 7)]);
        // A global id is unique.
        assert!(conn
            .execute("INSERT INTO play_events(track_id, started_at, listened_ms, device, seq) VALUES ('a', 1, 1, ?1, 7)", [&device])
            .is_err());
        // Ids never go back, even after the last row is gone: new local plays continue after 7.
        conn.execute("DELETE FROM play_events WHERE id = 7", []).unwrap();
        crate::stats::record(&conn, &mixtape_core::TrackId::from("a"), 30, 3).unwrap();
        let last: (i64, i64) =
            conn.query_row("SELECT id, seq FROM play_events WHERE started_at = 30", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(last, (8, 8));
        // Reopening keeps the same id.
        drop(conn);
        let conn = open_writer(&path).unwrap();
        assert_eq!(crate::settings::device_id(&conn).unwrap(), device);
    }

    #[test]
    fn fts_follows_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_writer(&dir.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO tracks(id, title, search_text) VALUES ('a', 'Take on Me', 'take on me a-ha')",
            [],
        )
        .unwrap();
        assert_eq!(count(&conn, "\"a-ha\""), 1);
        assert_eq!(count(&conn, "\"kom\""), 0);
        conn.execute("UPDATE tracks SET search_text = 'kokomo' WHERE id = 'a'", []).unwrap();
        assert_eq!(count(&conn, "\"kom\""), 1);
        conn.execute("DELETE FROM tracks WHERE id = 'a'", []).unwrap();
        assert_eq!(count(&conn, "\"kom\""), 0);
    }
}
