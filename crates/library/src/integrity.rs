//! Startup check: library tracks whose file is gone become `broken`; cache entries whose
//! file is gone are forgotten.

use crate::repo::set_state;
use crate::{Db, Result};
use mixtape_core::{TrackId, TrackState};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    pub broken: usize,
    pub dropped_cache: usize,
}

pub fn check(db: &Db, data_root: &Path) -> Result<IntegrityReport> {
    let files: Vec<(String, String, String)> = db.read(|c| {
        let mut stmt = c.prepare("SELECT track_id, path, location FROM media_files")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })?;
    // One directory listing per folder instead of a `stat` per file: with 20k tracks that is
    // the difference between ~1.3 s and a few ms of startup.
    let mut listed: HashMap<PathBuf, HashSet<OsString>> = HashMap::new();
    let mut exists = |full: &Path| -> bool {
        let (Some(dir), Some(name)) = (full.parent(), full.file_name()) else { return false };
        listed
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                std::fs::read_dir(dir)
                    .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| e.file_name()).collect())
                    .unwrap_or_default()
            })
            .contains(name)
    };
    let missing: Vec<(String, String)> = files
        .into_iter()
        .filter(|(_, path, _)| !exists(&data_root.join(path)))
        .map(|(id, _, loc)| (id, loc))
        .collect();
    if missing.is_empty() {
        return Ok(IntegrityReport::default());
    }
    db.write(move |t| {
        let mut report = IntegrityReport::default();
        for (id, loc) in &missing {
            let id = TrackId(id.clone());
            if loc == "library" {
                set_state(t, &id, TrackState::Broken)?;
                report.broken += 1;
            } else {
                t.execute("DELETE FROM media_files WHERE track_id = ?1", [id.as_str()])?;
                set_state(t, &id, TrackState::Remote)?;
                report.dropped_cache += 1;
            }
        }
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::{FileLocation, MediaFile, put_media_file, track, upsert_track};

    #[test]
    fn marks_missing_files() {
        let (dir, db) = db();
        std::fs::write(dir.path().join("ok.webm"), b"x").unwrap();
        db.write(|t| {
            for (id, path, loc) in [
                ("a", "ok.webm", FileLocation::Library),
                ("b", "gone.webm", FileLocation::Library),
                ("c", "gone2.webm", FileLocation::Cache),
            ] {
                upsert_track(t, &meta(id, id, ("UC1", "X"), None))?;
                put_media_file(
                    t,
                    &MediaFile {
                        track_id: TrackId::from(id),
                        path: path.into(),
                        container: "webm".into(),
                        itag: None,
                        size_bytes: 1,
                        sample_rate: None,
                        location: loc,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
        let r = check(&db, dir.path()).unwrap();
        assert_eq!(r, IntegrityReport { broken: 1, dropped_cache: 1 });
        let state = |id: &str| db.read(|c| track(c, &TrackId::from(id))).unwrap().unwrap().state;
        assert_eq!(state("a"), TrackState::Downloaded);
        assert_eq!(state("b"), TrackState::Broken);
        assert_eq!(state("c"), TrackState::Remote);
    }
}
