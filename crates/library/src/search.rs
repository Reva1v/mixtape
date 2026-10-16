//! Instant search over the library: substring match on normalised text, also trying the
//! query as if it was typed in the other keyboard layout.

use crate::Result;
use crate::normalize::{normalize, swap_layout};
use mixtape_core::{AlbumId, ArtistId, PlaylistId, TrackId};
use rusqlite::{Connection, params};

#[derive(Debug, Default, Clone)]
pub struct SearchIds {
    pub tracks: Vec<TrackId>,
    pub albums: Vec<AlbumId>,
    pub artists: Vec<ArtistId>,
    pub playlists: Vec<PlaylistId>,
}

/// Quotes a string for an FTS5 MATCH expression.
fn fts_phrase(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Escapes `%`, `_` and `\` for a LIKE pattern with `ESCAPE '\'`.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub fn search(conn: &Connection, query: &str, limit: usize) -> Result<SearchIds> {
    let q = normalize(query);
    if q.is_empty() {
        return Ok(SearchIds::default());
    }
    let alt = swap_layout(&q).map(|s| normalize(&s)).filter(|s| *s != q);
    let mut out = SearchIds::default();
    for variant in std::iter::once(&q).chain(alt.as_ref()) {
        let room = limit.saturating_sub(out.tracks.len());
        if room == 0 {
            break;
        }
        for id in tracks(conn, variant, room)? {
            if !out.tracks.contains(&id) {
                out.tracks.push(id);
            }
        }
        push_unique(&mut out.albums, like_ids(conn, "albums", variant, limit, AlbumId, LIBRARY_ALBUMS)?, limit);
        push_unique(&mut out.artists, like_ids(conn, "artists", variant, limit, ArtistId, LIBRARY_ARTISTS)?, limit);
        push_unique(&mut out.playlists, like_ids(conn, "playlists", variant, limit, PlaylistId, "1")?, limit);
    }
    Ok(out)
}

fn push_unique<T: PartialEq>(dst: &mut Vec<T>, src: Vec<T>, limit: usize) {
    for x in src {
        if dst.len() >= limit {
            break;
        }
        if !dst.contains(&x) {
            dst.push(x);
        }
    }
}

const LIBRARY_ALBUMS: &str =
    "EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = x.id AND t.state IN ('downloaded', 'broken'))";
const LIBRARY_ARTISTS: &str = "(x.in_library = 1 OR EXISTS (SELECT 1 FROM track_artists ta JOIN tracks t ON t.id = ta.track_id
    WHERE ta.artist_id = x.id AND t.state IN ('downloaded', 'broken')))";

fn tracks(conn: &Connection, q: &str, limit: usize) -> Result<Vec<TrackId>> {
    // Trigram FTS needs at least three characters; shorter queries match word starts.
    let rows: Vec<String> = if q.chars().count() >= 3 {
        let mut stmt = conn.prepare_cached(
            "SELECT t.id FROM tracks_fts f JOIN tracks t ON t.rowid = f.rowid
             LEFT JOIN play_stats p ON p.track_id = t.id
             WHERE tracks_fts MATCH ?1 AND t.state IN ('downloaded', 'broken')
             ORDER BY (t.search_text LIKE ?2 ESCAPE '\\') DESC, COALESCE(p.play_count, 0) DESC
             LIMIT ?3",
        )?;
        let prefix = format!("{}%", like_escape(q));
        stmt.query_map(params![fts_phrase(q), prefix, limit as i64], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    } else {
        let mut stmt = conn.prepare_cached(
            "SELECT t.id FROM tracks t LEFT JOIN play_stats p ON p.track_id = t.id
             WHERE t.state IN ('downloaded', 'broken')
               AND (t.search_text LIKE ?1 ESCAPE '\\' OR t.search_text LIKE ?2 ESCAPE '\\')
             ORDER BY (t.search_text LIKE ?1 ESCAPE '\\') DESC, COALESCE(p.play_count, 0) DESC
             LIMIT ?3",
        )?;
        let e = like_escape(q);
        stmt.query_map(params![format!("{e}%"), format!("% {e}%"), limit as i64], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    Ok(rows.into_iter().map(TrackId).collect())
}

fn like_ids<T>(
    conn: &Connection,
    table: &str,
    q: &str,
    limit: usize,
    wrap: impl Fn(String) -> T,
    filter: &str,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT x.id FROM {table} x WHERE {filter} AND x.search_text LIKE ?1 ESCAPE '\\'
         ORDER BY (x.search_text LIKE ?2 ESCAPE '\\') DESC LIMIT ?3"
    ))?;
    let e = like_escape(q);
    let rows = stmt.query_map(params![format!("%{e}%"), format!("{e}%"), limit as i64], |r| {
        r.get::<_, String>(0)
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(wrap(r?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::{set_state, upsert_track};
    use mixtape_core::TrackState;
    use std::time::Instant;

    fn ids(v: &[TrackId]) -> Vec<&str> {
        v.iter().map(|i| i.as_str()).collect()
    }

    #[test]
    fn finds_substrings_short_queries_and_wrong_layout() {
        let (_d, db) = db();
        db.write(|t| {
            upsert_track(t, &meta("1", "Mark of the Beast", ("UCa", "Mark James"), None))?;
            upsert_track(t, &meta("2", "Ёлка", ("UCb", "Кино"), None))?;
            upsert_track(t, &meta("3", "Remote", ("UCc", "Mark Remote"), None))?;
            for id in ["1", "2"] {
                set_state(t, &TrackId::from(id), TrackState::Downloaded)?;
            }
            Ok(())
        })
        .unwrap();
        let r = db.read(|c| search(c, "james", 10)).unwrap();
        assert_eq!(ids(&r.tracks), ["1"]);
        assert_eq!(r.artists.len(), 1);
        let r = db.read(|c| search(c, "ьфкл", 10)).unwrap(); // "mark" typed in Russian layout
        assert_eq!(ids(&r.tracks), ["1"]);
        let r = db.read(|c| search(c, "елк", 10)).unwrap(); // ё folded to е
        assert_eq!(ids(&r.tracks), ["2"]);
        let r = db.read(|c| search(c, "ки", 10)).unwrap(); // short query: word start
        assert_eq!(ids(&r.tracks), ["2"]);
        let r = db.read(|c| search(c, "100%", 10)).unwrap();
        assert!(r.tracks.is_empty());
    }

    #[test]
    fn search_20k_is_fast() {
        let (_d, db) = db();
        db.write(|t| {
            for i in 0..20_000 {
                let id = format!("id{i:05}");
                upsert_track(
                    t,
                    &meta(&id, &format!("Song number {i} — песня"), (&format!("UC{}", i % 900), &format!("Artist {}", i % 900)), Some((&format!("MP{}", i % 1500), &format!("Album {}", i % 1500)))),
                )?;
                set_state(t, &TrackId(id), TrackState::Downloaded)?;
            }
            Ok(())
        })
        .unwrap();
        for q in ["song number 1234", "песн", "artist 77", "ab", "ыщтп"] {
            let start = Instant::now();
            let r = db.read(|c| search(c, q, 30)).unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            assert!(!r.tracks.is_empty() || q == "ab", "{q}");
            // Debug builds are slower; the budget (≤50 ms) is for release.
            assert!(ms < 250.0, "{q}: {ms:.1} ms");
            eprintln!("search {q:?}: {ms:.1} ms, {} tracks", r.tracks.len());
        }
    }
}
