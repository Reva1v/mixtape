//! Stored results of the audio analysis for smart transitions (`track_analysis`, migration 007).
//! The analysis itself lives in `mixtape_audio::analysis`; this is plain storage.

use crate::Result;
use crate::repo::now_ms;
use mixtape_core::TrackId;
use rusqlite::{Connection, OptionalExtension, params};

/// One analysed track, as `mixtape_audio::TrackAnalysis` describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredAnalysis {
    pub version: u32,
    pub bpm: f64,
    pub beat_ms: f64,
    /// `0..12` major keys by tonic, `12..24` minor.
    pub key: Option<u8>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub intro_end_ms: u64,
    pub outro_start_ms: u64,
}

pub fn get(conn: &Connection, id: &TrackId) -> Result<Option<StoredAnalysis>> {
    Ok(conn
        .query_row(
            "SELECT version, bpm, beat_ms, key, start_ms, end_ms, intro_end_ms, outro_start_ms
             FROM track_analysis WHERE track_id = ?1",
            [id.as_str()],
            |r| {
                Ok(StoredAnalysis {
                    version: r.get(0)?,
                    bpm: r.get(1)?,
                    beat_ms: r.get(2)?,
                    key: r.get(3)?,
                    start_ms: r.get::<_, i64>(4)? as u64,
                    end_ms: r.get::<_, i64>(5)? as u64,
                    intro_end_ms: r.get::<_, i64>(6)? as u64,
                    outro_start_ms: r.get::<_, i64>(7)? as u64,
                })
            },
        )
        .optional()?)
}

pub fn set(conn: &Connection, id: &TrackId, a: &StoredAnalysis) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO track_analysis(track_id, version, bpm, beat_ms, key, start_ms, end_ms,
            intro_end_ms, outro_start_ms, analyzed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id.as_str(),
            a.version,
            a.bpm,
            a.beat_ms,
            a.key,
            a.start_ms as i64,
            a.end_ms as i64,
            a.intro_end_ms as i64,
            a.outro_start_ms as i64,
            now_ms()
        ],
    )?;
    Ok(())
}

/// Whether `id` has an audio file but no analysis of `version` yet; its file path (relative
/// to the data root) if so.
pub fn needs(conn: &Connection, id: &TrackId, version: u32) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT m.path FROM media_files m
             LEFT JOIN track_analysis a ON a.track_id = m.track_id
             WHERE m.track_id = ?1 AND (a.version IS NULL OR a.version <> ?2)",
            params![id.as_str(), version],
            |r| r.get(0),
        )
        .optional()?)
}

/// Up to `limit` tracks with an audio file and no analysis of `version`, library files first,
/// then the most recently played cached streams. `(track id, relative path)`.
pub fn pending(conn: &Connection, version: u32, limit: usize) -> Result<Vec<(TrackId, String)>> {
    let mut stmt = conn.prepare(
        "SELECT m.track_id, m.path FROM media_files m
         LEFT JOIN track_analysis a ON a.track_id = m.track_id
         WHERE a.version IS NULL OR a.version <> ?1
         ORDER BY m.location = 'cache', m.last_access DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![version, limit as i64], |r| {
        Ok((TrackId::from(r.get::<_, String>(0)?.as_str()), r.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::upsert_track;

    fn sample(version: u32) -> StoredAnalysis {
        StoredAnalysis {
            version,
            bpm: 123.5,
            beat_ms: 210.25,
            key: Some(21),
            start_ms: 100,
            end_ms: 179_000,
            intro_end_ms: 16_000,
            outro_start_ms: 160_000,
        }
    }

    #[test]
    fn stores_and_finds_what_is_left_to_analyse() {
        let (_d, db) = db();
        db.write(|t| {
            for (id, location, access) in [("a", "library", 1), ("b", "cache", 3), ("c", "library", 2)] {
                upsert_track(t, &meta(id, id, ("UCx", "X"), None))?;
                t.execute(
                    "INSERT INTO media_files(track_id, path, container, size_bytes, last_access, location)
                     VALUES (?1, ?2, 'webm', 1, ?3, ?4)",
                    params![id, format!("music/{id}.webm"), access, location],
                )?;
            }
            upsert_track(t, &meta("remote", "R", ("UCx", "X"), None))?;
            Ok(())
        })
        .unwrap();
        let ids = |v| -> Vec<String> {
            db.read(|c| pending(c, v, 10))
                .unwrap()
                .into_iter()
                .map(|(id, _)| id.0)
                .collect()
        };
        assert_eq!(
            ids(1),
            ["c", "a", "b"],
            "library first, recent first; no file, no analysis"
        );

        let a = TrackId::from("a");
        let a2 = a.clone();
        db.write(move |t| set(t, &a2, &sample(1))).unwrap();
        assert_eq!(db.read(|c| get(c, &a)).unwrap(), Some(sample(1)));
        assert_eq!(ids(1), ["c", "b"]);
        assert_eq!(db.read(|c| needs(c, &a, 1)).unwrap(), None);
        assert_eq!(
            db.read(|c| needs(c, &a, 2)).unwrap().as_deref(),
            Some("music/a.webm")
        );
        assert_eq!(
            db.read(|c| needs(c, &TrackId::from("remote"), 1)).unwrap(),
            None
        );
        // A newer analyser redoes everything.
        assert_eq!(ids(2), ["c", "a", "b"]);
    }
}
