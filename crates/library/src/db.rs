//! Opening connections and applying migrations.

use crate::Result;
use rusqlite::{Connection, OpenFlags};
use rusqlite_migration::{M, Migrations};
use std::path::Path;
use std::time::Duration;

const MIGRATIONS_SLICE: &[M<'static>] = &[M::up(include_str!("../migrations/001_init.sql"))];

fn migrations() -> Migrations<'static> {
    Migrations::from_slice(MIGRATIONS_SLICE)
}

/// Opens the writer connection and brings the schema up to date.
pub fn open_writer(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -16_000)?; // ~16 MB
    conn.busy_timeout(Duration::from_secs(5))?;
    migrations().to_latest(&mut conn)?;
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
