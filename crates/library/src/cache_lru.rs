//! Stream cache: files under `<root>/cache/` recorded as `media_files.location = 'cache'`,
//! evicted least-recently-used once their total passes the limit, and promoted into
//! `library/` without a new download (autosave, like, «Скачать»).

use crate::repo::{now_ms, set_state};
use crate::{Db, Result, settings};
use mixtape_core::{TrackId, TrackState};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// Setting: cache limit in bytes.
pub const CACHE_LIMIT_KEY: &str = "cache_limit_bytes";
pub const DEFAULT_CACHE_LIMIT: u64 = 5 * 1024 * 1024 * 1024;
/// Setting: full plays before a cached track is saved to the library; JSON `null` turns
/// autosave off.
pub const AUTOSAVE_KEY: &str = "autosave_plays";
pub const DEFAULT_AUTOSAVE_PLAYS: u32 = 3;

pub fn cache_limit(db: &Db) -> Result<u64> {
    settings::get_or(db, CACHE_LIMIT_KEY, DEFAULT_CACHE_LIMIT)
}

/// `None` when autosave is off.
pub fn autosave_threshold(db: &Db) -> Result<Option<u32>> {
    Ok(settings::get::<Option<u32>>(db, AUTOSAVE_KEY)?.unwrap_or(Some(DEFAULT_AUTOSAVE_PLAYS)))
}

/// Whether a track with this many full plays should be saved. `None` or `Some(0)` = off.
pub fn should_autosave(full_play_count: u32, threshold: Option<u32>) -> bool {
    matches!(threshold, Some(n) if n > 0 && full_play_count >= n)
}

/// Total size of cached files.
pub fn cache_size(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM media_files WHERE location = 'cache'",
        [],
        |r| r.get(0),
    )?;
    Ok(n.max(0) as u64)
}

/// Marks the track's file as just used.
pub fn touch(conn: &Connection, id: &TrackId) -> Result<()> {
    conn.execute(
        "UPDATE media_files SET last_access = ?2 WHERE track_id = ?1",
        params![id.as_str(), now_ms()],
    )?;
    Ok(())
}

/// Records a streamed file under `cache/` and moves a `remote` track to `cached`.
/// A track that already has a library file keeps it and its state; then nothing is
/// recorded and `false` comes back, so the caller can delete the redundant cache file.
pub fn register_cached(
    conn: &Connection,
    id: &TrackId,
    rel_path: &str,
    container: &str,
    itag: Option<u32>,
    size: u64,
) -> Result<bool> {
    let changed = conn.execute(
        "INSERT INTO media_files(track_id, path, container, itag, size_bytes, last_access, location)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'cache')
         ON CONFLICT(track_id) DO UPDATE SET path = excluded.path, container = excluded.container,
            itag = excluded.itag, size_bytes = excluded.size_bytes, sample_rate = NULL,
            lufs = NULL, true_peak = NULL, last_access = excluded.last_access
         WHERE media_files.location = 'cache'",
        params![id.as_str(), rel_path, container, itag, size as i64, now_ms()],
    )?;
    if changed == 0 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE tracks SET state = 'cached' WHERE id = ?1 AND state = 'remote'",
        [id.as_str()],
    )?;
    Ok(true)
}

/// Deletes least-recently-used cache files until the cache fits in `limit_bytes`.
/// Tracks in `protect` (playing, prefetching) are never touched; a file that cannot be
/// deleted (locked) is skipped. Evicted tracks go `cached → remote`.
pub fn evict(
    conn: &Connection,
    root: &Path,
    limit_bytes: u64,
    protect: &[TrackId],
) -> Result<Vec<TrackId>> {
    let mut total = cache_size(conn)?;
    if total <= limit_bytes {
        return Ok(Vec::new());
    }
    let files: Vec<(String, String, i64)> = conn
        .prepare(
            "SELECT track_id, path, size_bytes FROM media_files WHERE location = 'cache'
             ORDER BY last_access, track_id",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut evicted = Vec::new();
    for (id, path, size) in files {
        if total <= limit_bytes {
            break;
        }
        if protect.iter().any(|p| p.as_str() == id) {
            continue;
        }
        match std::fs::remove_file(root.join(&path)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(%id, %path, error = %e, "cache file not evicted");
                continue;
            }
        }
        forget_cached(conn, &id)?;
        total = total.saturating_sub(size.max(0) as u64);
        evicted.push(TrackId(id));
    }
    Ok(evicted)
}

fn forget_cached(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM media_files WHERE track_id = ?1 AND location = 'cache'",
        [id],
    )?;
    conn.execute(
        "UPDATE tracks SET state = 'remote' WHERE id = ?1 AND state = 'cached'",
        [id],
    )?;
    Ok(())
}

/// Moves the track's cached file into `library/` (rename, else copy and delete) and makes
/// it `downloaded`. Returns `false` when there is no cache entry; a cache entry whose file
/// is gone is forgotten (`cached → remote`) and also gives `false`.
pub fn promote(conn: &Connection, root: &Path, id: &TrackId) -> Result<bool> {
    let path: Option<String> = conn
        .query_row(
            "SELECT path FROM media_files WHERE track_id = ?1 AND location = 'cache'",
            [id.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(path) = path else { return Ok(false) };
    let from = root.join(&path);
    if !from.is_file() {
        forget_cached(conn, id.as_str())?;
        return Ok(false);
    }
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| id.as_str().to_owned());
    let rel = format!("library/{name}");
    let to = root.join(&rel);
    std::fs::create_dir_all(root.join("library"))?;
    if std::fs::rename(&from, &to).is_err() {
        std::fs::copy(&from, &to)?;
        if let Err(e) = std::fs::remove_file(&from) {
            tracing::warn!(%id, error = %e, "cache copy left behind after promote");
        }
    }
    conn.execute(
        "UPDATE media_files SET path = ?2, location = 'library' WHERE track_id = ?1",
        params![id.as_str(), rel],
    )?;
    set_state(conn, id, TrackState::Downloaded)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::{FileLocation, media_file, track, upsert_track};

    fn add_cached(conn: &Connection, root: &Path, id: &str, size: u64, at: i64) {
        upsert_track(conn, &meta(id, id, ("UC1", "X"), None)).unwrap();
        let rel = format!("cache/{id}.webm");
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(root.join(&rel), vec![0u8; size as usize]).unwrap();
        assert!(register_cached(conn, &TrackId::from(id), &rel, "webm", Some(251), size).unwrap());
        conn.execute(
            "UPDATE media_files SET last_access = ?2 WHERE track_id = ?1",
            params![id, at],
        )
        .unwrap();
    }

    fn state(db: &Db, id: &str) -> TrackState {
        db.read(|c| track(c, &TrackId::from(id)))
            .unwrap()
            .unwrap()
            .state
    }

    #[test]
    fn evicts_oldest_but_spares_protected() {
        let (dir, db) = db();
        let root = dir.path().to_owned();
        let r = root.clone();
        let evicted = db
            .write(move |t| {
                add_cached(t, &r, "a", 10, 1);
                add_cached(t, &r, "b", 10, 2);
                add_cached(t, &r, "c", 10, 3);
                add_cached(t, &r, "d", 10, 4);
                touch(t, &TrackId::from("b"))?;
                assert_eq!(cache_size(t)?, 40);
                // "a" is oldest but protected; "c" goes next, then "d" ("b" was touched).
                evict(t, &r, 20, &[TrackId::from("a")])
            })
            .unwrap();
        assert_eq!(evicted, vec![TrackId::from("c"), TrackId::from("d")]);
        assert!(root.join("cache/a.webm").is_file());
        assert!(!root.join("cache/c.webm").exists());
        assert_eq!(state(&db, "c"), TrackState::Remote);
        assert_eq!(state(&db, "a"), TrackState::Cached);
        assert_eq!(db.read(cache_size).unwrap(), 20);
        assert!(
            db.read(|c| media_file(c, &TrackId::from("d")))
                .unwrap()
                .is_none()
        );
        // Already under the limit: nothing to do.
        let r = root.clone();
        assert!(db.write(move |t| evict(t, &r, 20, &[])).unwrap().is_empty());
    }

    #[test]
    fn promote_moves_file_into_library() {
        let (dir, db) = db();
        let root = dir.path().to_owned();
        let r = root.clone();
        let (moved, again, none) = db
            .write(move |t| {
                add_cached(t, &r, "a", 5, 1);
                upsert_track(t, &meta("z", "Z", ("UC1", "X"), None))?;
                Ok((
                    promote(t, &r, &TrackId::from("a"))?,
                    promote(t, &r, &TrackId::from("a"))?,
                    promote(t, &r, &TrackId::from("z"))?,
                ))
            })
            .unwrap();
        assert!(moved && !again && !none);
        assert!(!root.join("cache/a.webm").exists());
        assert_eq!(std::fs::read(root.join("library/a.webm")).unwrap().len(), 5);
        let m = db
            .read(|c| media_file(c, &TrackId::from("a")))
            .unwrap()
            .unwrap();
        assert_eq!(
            (m.path.as_str(), m.location),
            ("library/a.webm", FileLocation::Library)
        );
        assert_eq!(state(&db, "a"), TrackState::Downloaded);
        let added: Option<i64> = db
            .read(|c| {
                Ok(
                    c.query_row("SELECT added_at FROM tracks WHERE id = 'a'", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap();
        assert!(added.is_some());
        assert_eq!(db.read(cache_size).unwrap(), 0);
    }

    #[test]
    fn promote_forgets_missing_file() {
        let (dir, db) = db();
        let r = dir.path().to_owned();
        let moved = db
            .write(move |t| {
                add_cached(t, &r, "a", 5, 1);
                std::fs::remove_file(r.join("cache/a.webm"))?;
                promote(t, &r, &TrackId::from("a"))
            })
            .unwrap();
        assert!(!moved);
        assert_eq!(state(&db, "a"), TrackState::Remote);
    }

    #[test]
    fn caching_never_downgrades_library_tracks() {
        let (dir, db) = db();
        let r = dir.path().to_owned();
        let registered = db
            .write(move |t| {
                add_cached(t, &r, "a", 5, 1);
                promote(t, &r, &TrackId::from("a"))?;
                register_cached(t, &TrackId::from("a"), "cache/a.webm", "webm", Some(251), 5)
            })
            .unwrap();
        assert!(!registered);
        assert_eq!(state(&db, "a"), TrackState::Downloaded);
        let m = db
            .read(|c| media_file(c, &TrackId::from("a")))
            .unwrap()
            .unwrap();
        assert_eq!(m.location, FileLocation::Library);
    }

    #[test]
    fn autosave_rule_and_settings() {
        assert!(!should_autosave(2, Some(3)));
        assert!(should_autosave(3, Some(3)));
        assert!(!should_autosave(100, None));
        assert!(!should_autosave(100, Some(0)));
        let (_dir, db) = db();
        assert_eq!(cache_limit(&db).unwrap(), DEFAULT_CACHE_LIMIT);
        assert_eq!(
            autosave_threshold(&db).unwrap(),
            Some(DEFAULT_AUTOSAVE_PLAYS)
        );
        settings::set(&db, AUTOSAVE_KEY, &None::<u32>).unwrap();
        assert_eq!(autosave_threshold(&db).unwrap(), None);
        settings::set(&db, AUTOSAVE_KEY, &Some(5u32)).unwrap();
        assert_eq!(autosave_threshold(&db).unwrap(), Some(5));
    }
}
