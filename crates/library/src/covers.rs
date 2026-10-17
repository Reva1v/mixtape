//! Which cover-store keys the database still refers to, for the cover cleanup.

use crate::Result;
use rusqlite::Connection;
use std::collections::HashSet;

/// Cover keys in use.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeysInUse {
    /// Every key some row refers to.
    pub any: HashSet<String>,
    /// Keys of library items, whose 1024 px size is kept: tracks in the library or queued
    /// for download, their albums, saved albums, library artists, playlists.
    pub full: HashSet<String>,
}

const ANY: &str = "SELECT cover_key FROM tracks WHERE cover_key IS NOT NULL
    UNION SELECT cover_key FROM albums WHERE cover_key IS NOT NULL
    UNION SELECT image_key FROM artists WHERE image_key IS NOT NULL
    UNION SELECT cover_key FROM playlists WHERE cover_key IS NOT NULL";

const FULL: &str = "WITH lib AS (
        SELECT t.id, t.album_id, t.cover_key FROM tracks t
        WHERE t.state IN ('downloaded', 'broken')
           OR EXISTS (SELECT 1 FROM download_jobs j WHERE j.track_id = t.id)
    )
    SELECT cover_key FROM lib WHERE cover_key IS NOT NULL
    UNION SELECT al.cover_key FROM albums al
        WHERE al.cover_key IS NOT NULL
          AND (al.in_library = 1 OR EXISTS (SELECT 1 FROM lib WHERE lib.album_id = al.id))
    UNION SELECT a.image_key FROM artists a
        WHERE a.image_key IS NOT NULL
          AND (a.in_library = 1
               OR EXISTS (SELECT 1 FROM track_artists ta JOIN lib ON lib.id = ta.track_id WHERE ta.artist_id = a.id)
               OR EXISTS (SELECT 1 FROM album_artists aa JOIN lib ON lib.album_id = aa.album_id WHERE aa.artist_id = a.id))
    UNION SELECT cover_key FROM playlists WHERE cover_key IS NOT NULL";

pub fn keys_in_use(conn: &Connection) -> Result<KeysInUse> {
    let collect = |sql: &str| -> Result<HashSet<String>> {
        let mut stmt = conn.prepare(sql)?;
        let keys = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(keys)
    };
    Ok(KeysInUse { any: collect(ANY)?, full: collect(FULL)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::{set_state, upsert_track};
    use mixtape_core::{TrackId, TrackState};

    #[test]
    fn library_items_keep_the_large_size() {
        let (_dir, db) = db();
        let keys = db
            .write(|t| {
                let mut owned = meta("own", "Own", ("UCa", "A"), Some(("MPREown", "Own LP")));
                owned.cover_key = Some("MPREown".into());
                upsert_track(t, &owned)?;
                set_state(t, &TrackId::from("own"), TrackState::Downloaded)?;
                let mut browsed = meta("brw", "Browsed", ("UCb", "B"), Some(("MPREbrw", "Other LP")));
                browsed.cover_key = Some("MPREbrw".into());
                upsert_track(t, &browsed)?;
                let mut queued = meta("que", "Queued", ("UCb", "B"), None);
                queued.cover_key = Some("que".into());
                upsert_track(t, &queued)?;
                crate::jobs::enqueue(t, &TrackId::from("que"), "sync")?;
                t.execute("UPDATE albums SET cover_key = 'MPREown' WHERE id = 'MPREown'", [])?;
                t.execute("UPDATE albums SET cover_key = 'MPREbrw' WHERE id = 'MPREbrw'", [])?;
                t.execute("UPDATE artists SET image_key = 'UCa' WHERE id = 'UCa'", [])?;
                t.execute("UPDATE artists SET image_key = 'UCb' WHERE id = 'UCb'", [])?;
                keys_in_use(t)
            })
            .unwrap();
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        assert_eq!(keys.any, set(&["MPREown", "MPREbrw", "que", "UCa", "UCb"]));
        // UCb is credited on a queued track only through track_artists of `que`.
        assert_eq!(keys.full, set(&["MPREown", "que", "UCa", "UCb"]));
    }
}
