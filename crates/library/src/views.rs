//! Sorted id lists behind the UI's virtual lists. The UI asks for a view once, gets its
//! id and size, then pages rows by offset; 20k rows never cross IPC at once.

use crate::Result;
use mixtape_core::{AlbumId, ArtistId, PlaylistId, TrackId};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ViewKind {
    Tracks,
    Liked,
    Albums,
    Artists,
    Playlists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ViewSort {
    /// Newest first (added to the library / liked).
    #[default]
    Recent,
    Title,
    Artist,
    Album,
    Year,
}

#[derive(Debug, Clone)]
pub enum ViewIds {
    Tracks(Vec<TrackId>),
    Albums(Vec<AlbumId>),
    Artists(Vec<ArtistId>),
    Playlists(Vec<PlaylistId>),
}

impl ViewIds {
    pub fn len(&self) -> usize {
        match self {
            Self::Tracks(v) => v.len(),
            Self::Albums(v) => v.len(),
            Self::Artists(v) => v.len(),
            Self::Playlists(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

const IN_LIBRARY: &str = "t.state IN ('downloaded', 'broken')";

fn collect<T>(conn: &Connection, sql: &str, args: &[String], wrap: impl Fn(String) -> T) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(wrap(r?));
    }
    Ok(out)
}

const FIRST_ARTIST: &str =
    "(SELECT name FROM track_artists ta WHERE ta.track_id = t.id ORDER BY position LIMIT 1)";

/// Builds the ordered id list for a library screen.
pub fn build(conn: &Connection, kind: ViewKind, sort: ViewSort) -> Result<ViewIds> {
    build_filtered(conn, kind, sort, "")
}

/// `AND`-ed `LIKE` conditions on `column` for every word of `filter` (normalized like the
/// stored `search_text`, so case, «ё» and diacritics don't matter), with their arguments.
fn word_filter(column: &str, filter: &str) -> (String, Vec<String>) {
    let norm = crate::normalize::normalize(filter);
    let args: Vec<String> = norm
        .split_whitespace()
        .map(|w| format!("%{}%", w.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")))
        .collect();
    let sql = args.iter().map(|_| format!(" AND {column} LIKE ? ESCAPE '\\'")).collect();
    (sql, args)
}

/// [`build`] narrowed to rows matching every word of `filter` (the library's own search box).
pub fn build_filtered(conn: &Connection, kind: ViewKind, sort: ViewSort, filter: &str) -> Result<ViewIds> {
    Ok(match kind {
        ViewKind::Tracks | ViewKind::Liked => {
            let scope = if kind == ViewKind::Liked { "t.liked = 1" } else { IN_LIBRARY };
            let order = match (kind, sort) {
                (ViewKind::Liked, ViewSort::Recent) => "t.liked_at DESC".to_owned(),
                (_, ViewSort::Recent) => "t.added_at DESC, t.rowid DESC".to_owned(),
                (_, ViewSort::Title) => "t.title COLLATE NOCASE".to_owned(),
                (_, ViewSort::Artist) => format!("{FIRST_ARTIST} COLLATE NOCASE, al.title, t.track_no"),
                (_, ViewSort::Album | ViewSort::Year) => {
                    "al.title COLLATE NOCASE, t.track_no, t.title".to_owned()
                }
            };
            let (words, args) = word_filter("t.search_text", filter);
            let sql = format!(
                "SELECT t.id FROM tracks t LEFT JOIN albums al ON al.id = t.album_id WHERE {scope}{words} ORDER BY {order}"
            );
            ViewIds::Tracks(collect(conn, &sql, &args, TrackId)?)
        }
        ViewKind::Albums => {
            let order = match sort {
                ViewSort::Title => "al.title COLLATE NOCASE",
                ViewSort::Artist => {
                    "(SELECT name FROM album_artists x WHERE x.album_id = al.id ORDER BY position LIMIT 1) COLLATE NOCASE, al.year"
                }
                ViewSort::Year => "al.year DESC, al.title",
                _ => "last_added DESC",
            };
            let (words, args) = word_filter("al.search_text", filter);
            let sql = format!(
                "SELECT al.id, MAX(t.added_at) AS last_added FROM albums al
                 JOIN tracks t ON t.album_id = al.id AND {IN_LIBRARY}
                 WHERE 1 = 1{words}
                 GROUP BY al.id ORDER BY {order}"
            );
            ViewIds::Albums(collect(conn, &sql, &args, AlbumId)?)
        }
        ViewKind::Artists => {
            let (words, args) = word_filter("ar.search_text", filter);
            let sql = format!(
                "SELECT ar.id FROM artists ar WHERE (ar.in_library = 1 OR EXISTS (
                    SELECT 1 FROM track_artists ta JOIN tracks t ON t.id = ta.track_id
                    WHERE ta.artist_id = ar.id AND {IN_LIBRARY})){words}
                 ORDER BY ar.name COLLATE NOCASE"
            );
            ViewIds::Artists(collect(conn, &sql, &args, ArtistId)?)
        }
        ViewKind::Playlists => {
            let order = if sort == ViewSort::Title { "title COLLATE NOCASE" } else { "rowid DESC" };
            let (words, args) = word_filter("search_text", filter);
            let sql = format!("SELECT id FROM playlists WHERE 1 = 1{words} ORDER BY origin = 'ytm', {order}");
            ViewIds::Playlists(collect(conn, &sql, &args, PlaylistId)?)
        }
    })
}

/// Recently built views, kept so paging does not re-run the query. Old views are
/// dropped; the UI rebuilds a view when it gets `library:changed`.
pub struct ViewCache {
    next: AtomicU64,
    views: Mutex<VecDeque<(u64, ViewIds)>>,
}

/// The Library screen holds one view at a time and a dialog briefly a second, so three
/// is enough; each view may hold tens of thousands of ids.
const KEEP: usize = 3;

impl Default for ViewCache {
    fn default() -> Self {
        Self { next: AtomicU64::new(1), views: Mutex::new(VecDeque::new()) }
    }
}

impl ViewCache {
    pub fn insert(&self, ids: ViewIds) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut v = self.views.lock().expect("view cache poisoned");
        v.push_back((id, ids));
        while v.len() > KEEP {
            v.pop_front();
        }
        id
    }

    /// Clones out the requested window of a view.
    pub fn slice(&self, view: u64, offset: usize, limit: usize) -> Option<ViewIds> {
        let v = self.views.lock().expect("view cache poisoned");
        let (_, ids) = v.iter().find(|(id, _)| *id == view)?;
        fn win<T: Clone>(v: &[T], o: usize, l: usize) -> Vec<T> {
            v.iter().skip(o).take(l).cloned().collect()
        }
        Some(match ids {
            ViewIds::Tracks(x) => ViewIds::Tracks(win(x, offset, limit)),
            ViewIds::Albums(x) => ViewIds::Albums(win(x, offset, limit)),
            ViewIds::Artists(x) => ViewIds::Artists(win(x, offset, limit)),
            ViewIds::Playlists(x) => ViewIds::Playlists(win(x, offset, limit)),
        })
    }

    /// All track ids of a track view, for "play all" from a list.
    pub fn track_ids(&self, view: u64) -> Option<Vec<TrackId>> {
        let v = self.views.lock().expect("view cache poisoned");
        match v.iter().find(|(id, _)| *id == view)? {
            (_, ViewIds::Tracks(x)) => Some(x.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::{set_liked, set_state, upsert_track};
    use mixtape_core::TrackState;

    #[test]
    fn library_search_box_matches_every_word() {
        let (_d, db) = db();
        db.write(|t| {
            upsert_track(t, &meta("1", "Ёлка в лесу", ("UCa", "Alpha"), Some(("MPa", "Winter 100%"))))?;
            upsert_track(t, &meta("2", "Apple", ("UCb", "Beta"), Some(("MPb", "Album B"))))?;
            set_state(t, &TrackId::from("1"), TrackState::Downloaded)?;
            set_state(t, &TrackId::from("2"), TrackState::Downloaded)?;
            Ok(())
        })
        .unwrap();
        let ids = |kind, f: &str| match db.read(|c| build_filtered(c, kind, ViewSort::Title, f)).unwrap() {
            ViewIds::Tracks(v) => v.into_iter().map(|t| t.0).collect::<Vec<_>>(),
            ViewIds::Albums(v) => v.into_iter().map(|a| a.0).collect(),
            ViewIds::Artists(v) => v.into_iter().map(|a| a.0).collect(),
            ViewIds::Playlists(v) => v.into_iter().map(|p| p.0).collect(),
        };
        assert_eq!(ids(ViewKind::Tracks, "елка ALPHA"), ["1"], "case, ё and words in any order");
        assert_eq!(ids(ViewKind::Tracks, "100%"), ["1"], "% is literal");
        assert!(ids(ViewKind::Tracks, "_").is_empty(), "_ is literal");
        assert_eq!(ids(ViewKind::Tracks, ""), ["2", "1"]);
        assert_eq!(ids(ViewKind::Albums, "album"), ["MPb"]);
        assert_eq!(ids(ViewKind::Artists, "bet"), ["UCb"]);
    }

    #[test]
    fn library_views_filter_and_sort() {
        let (_d, db) = db();
        db.write(|t| {
            upsert_track(t, &meta("1", "Zebra", ("UCa", "Alpha"), Some(("MPa", "Album A"))))?;
            upsert_track(t, &meta("2", "Apple", ("UCb", "Beta"), Some(("MPb", "Album B"))))?;
            upsert_track(t, &meta("3", "Remote only", ("UCc", "Gamma"), None))?;
            set_state(t, &TrackId::from("1"), TrackState::Downloaded)?;
            set_state(t, &TrackId::from("2"), TrackState::Downloaded)?;
            set_liked(t, &TrackId::from("3"), true)?;
            Ok(())
        })
        .unwrap();
        let ids = |v: ViewIds| match v {
            ViewIds::Tracks(x) => x.into_iter().map(|i| i.0).collect::<Vec<_>>(),
            _ => panic!(),
        };
        let by_title = db.read(|c| build(c, ViewKind::Tracks, ViewSort::Title)).unwrap();
        assert_eq!(ids(by_title), ["2", "1"]);
        let liked = db.read(|c| build(c, ViewKind::Liked, ViewSort::Recent)).unwrap();
        assert_eq!(ids(liked), ["3"]);
        let albums = db.read(|c| build(c, ViewKind::Albums, ViewSort::Title)).unwrap();
        assert_eq!(albums.len(), 2);
        let artists = db.read(|c| build(c, ViewKind::Artists, ViewSort::Title)).unwrap();
        assert_eq!(artists.len(), 2); // Gamma has no library tracks

        let cache = ViewCache::default();
        let v = cache.insert(db.read(|c| build(c, ViewKind::Tracks, ViewSort::Title)).unwrap());
        assert_eq!(ids(cache.slice(v, 1, 10).unwrap()), ["1"]);
        assert!(cache.slice(v + 1, 0, 10).is_none());
    }
}
