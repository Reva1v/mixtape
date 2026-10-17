//! Reading and writing library entities. Functions take a `&Connection`; inside
//! `Db::write` a `&Transaction` derefs to one.

use crate::normalize::{normalize, track_search_text};
use crate::{Error, Result};
use mixtape_core::{
    Album, AlbumId, AlbumRef, AlbumType, Artist, ArtistId, ArtistRef, Playlist, PlaylistId,
    PlaylistOrigin, Track, TrackId, TrackState,
};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::collections::HashMap;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------- writes ----------

/// Metadata for a track as it comes from YouTube Music or a local import.
#[derive(Debug, Clone)]
pub struct TrackMeta {
    pub id: TrackId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub track_no: Option<u32>,
    pub duration_ms: u64,
    pub cover_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlbumMeta {
    pub id: AlbumId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub year: Option<u16>,
    pub album_type: AlbumType,
    pub cover_key: Option<String>,
    pub palette: Vec<String>,
    pub track_count: Option<u32>,
}

/// Inserts or refreshes a track's metadata. The state, like and dates are left alone
/// for existing rows; new rows start as `remote`.
pub fn upsert_track(conn: &Connection, t: &TrackMeta) -> Result<()> {
    let names: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
    let search = track_search_text(&t.title, &names, t.album.as_ref().map(|a| a.title.as_str()));
    if let Some(album) = &t.album {
        conn.execute(
            "INSERT INTO albums(id, title, search_text) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO NOTHING",
            params![album.id.as_str(), album.title, normalize(&album.title)],
        )?;
    }
    conn.execute(
        "INSERT INTO tracks(id, title, album_id, track_no, duration_ms, cover_key, search_text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            album_id = COALESCE(excluded.album_id, tracks.album_id),
            track_no = COALESCE(excluded.track_no, tracks.track_no),
            duration_ms = CASE WHEN excluded.duration_ms > 0 THEN excluded.duration_ms ELSE tracks.duration_ms END,
            cover_key = COALESCE(excluded.cover_key, tracks.cover_key),
            search_text = excluded.search_text",
        params![
            t.id.as_str(),
            t.title,
            t.album.as_ref().map(|a| a.id.as_str()),
            t.track_no,
            t.duration_ms as i64,
            t.cover_key,
            search
        ],
    )?;
    conn.execute("DELETE FROM track_artists WHERE track_id = ?1", [t.id.as_str()])?;
    for (pos, a) in t.artists.iter().enumerate() {
        if let Some(id) = &a.id {
            upsert_artist_name(conn, id, &a.name)?;
        }
        conn.execute(
            "INSERT INTO track_artists(track_id, position, artist_id, name) VALUES (?1, ?2, ?3, ?4)",
            params![t.id.as_str(), pos as i64, a.id.as_ref().map(|i| i.as_str()), a.name],
        )?;
    }
    Ok(())
}

fn upsert_artist_name(conn: &Connection, id: &ArtistId, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO artists(id, name, search_text) VALUES (?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, search_text = excluded.search_text",
        params![id.as_str(), name, normalize(name)],
    )?;
    Ok(())
}

pub fn upsert_artist(conn: &Connection, a: &Artist) -> Result<()> {
    conn.execute(
        "INSERT INTO artists(id, name, image_key, in_library, search_text) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name,
            image_key = COALESCE(excluded.image_key, artists.image_key),
            in_library = MAX(artists.in_library, excluded.in_library),
            search_text = excluded.search_text",
        params![a.id.as_str(), a.name, a.image_key, a.in_library, normalize(&a.name)],
    )?;
    Ok(())
}

pub fn upsert_album(conn: &Connection, a: &AlbumMeta) -> Result<()> {
    let names: Vec<&str> = a.artists.iter().map(|x| x.name.as_str()).collect();
    let search = normalize(&format!("{} {}", a.title, names.join(" ")));
    let palette = (!a.palette.is_empty()).then(|| serde_json::to_string(&a.palette)).transpose()?;
    conn.execute(
        "INSERT INTO albums(id, title, year, album_type, cover_key, palette, track_count, search_text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET title = excluded.title,
            year = COALESCE(excluded.year, albums.year),
            album_type = excluded.album_type,
            cover_key = COALESCE(excluded.cover_key, albums.cover_key),
            palette = COALESCE(excluded.palette, albums.palette),
            track_count = COALESCE(excluded.track_count, albums.track_count),
            search_text = excluded.search_text",
        params![
            a.id.as_str(),
            a.title,
            a.year,
            a.album_type.as_str(),
            a.cover_key,
            palette,
            a.track_count,
            search
        ],
    )?;
    conn.execute("DELETE FROM album_artists WHERE album_id = ?1", [a.id.as_str()])?;
    for (pos, ar) in a.artists.iter().enumerate() {
        if let Some(id) = &ar.id {
            upsert_artist_name(conn, id, &ar.name)?;
        }
        conn.execute(
            "INSERT INTO album_artists(album_id, position, artist_id, name) VALUES (?1, ?2, ?3, ?4)",
            params![a.id.as_str(), pos as i64, ar.id.as_ref().map(|i| i.as_str()), ar.name],
        )?;
    }
    Ok(())
}

pub fn set_state(conn: &Connection, id: &TrackId, state: TrackState) -> Result<()> {
    let added = state == TrackState::Downloaded;
    conn.execute(
        "UPDATE tracks SET state = ?2,
            added_at = CASE WHEN ?3 AND added_at IS NULL THEN ?4 ELSE added_at END
         WHERE id = ?1",
        params![id.as_str(), state.as_str(), added, now_ms()],
    )?;
    Ok(())
}

pub fn set_liked(conn: &Connection, id: &TrackId, liked: bool) -> Result<()> {
    conn.execute(
        "UPDATE tracks SET liked = ?2, liked_at = CASE WHEN ?2 THEN ?3 ELSE NULL END WHERE id = ?1",
        params![id.as_str(), liked, now_ms()],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct MediaFile {
    pub track_id: TrackId,
    /// Relative to the data directory.
    pub path: String,
    pub container: String,
    pub itag: Option<u32>,
    pub size_bytes: u64,
    pub sample_rate: Option<u32>,
    pub location: FileLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLocation {
    Cache,
    Library,
}

impl FileLocation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::Library => "library",
        }
    }
}

/// Records an audio file and moves the track to `cached` or `downloaded` accordingly.
pub fn put_media_file(conn: &Connection, f: &MediaFile) -> Result<()> {
    conn.execute(
        "INSERT INTO media_files(track_id, path, container, itag, size_bytes, sample_rate, last_access, location)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(track_id) DO UPDATE SET path = excluded.path, container = excluded.container,
            itag = excluded.itag, size_bytes = excluded.size_bytes, sample_rate = excluded.sample_rate,
            last_access = excluded.last_access, location = excluded.location",
        params![
            f.track_id.as_str(),
            f.path,
            f.container,
            f.itag,
            f.size_bytes as i64,
            f.sample_rate,
            now_ms(),
            f.location.as_str()
        ],
    )?;
    let state = match f.location {
        FileLocation::Cache => TrackState::Cached,
        FileLocation::Library => TrackState::Downloaded,
    };
    set_state(conn, &f.track_id, state)
}

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub path: String,
    pub container: String,
    pub location: FileLocation,
    pub lufs: Option<f64>,
    pub true_peak: Option<f64>,
}

pub fn media_file(conn: &Connection, id: &TrackId) -> Result<Option<MediaInfo>> {
    Ok(conn
        .query_row(
            "SELECT path, container, location, lufs, true_peak FROM media_files WHERE track_id = ?1",
            [id.as_str()],
            |r| {
                Ok(MediaInfo {
                    path: r.get(0)?,
                    container: r.get(1)?,
                    location: if r.get::<_, String>(2)? == "library" {
                        FileLocation::Library
                    } else {
                        FileLocation::Cache
                    },
                    lufs: r.get(3)?,
                    true_peak: r.get(4)?,
                })
            },
        )
        .optional()?)
}

pub fn set_loudness(conn: &Connection, id: &TrackId, lufs: f64, true_peak: f64) -> Result<()> {
    conn.execute(
        "UPDATE media_files SET lufs = ?2, true_peak = ?3 WHERE track_id = ?1",
        params![id.as_str(), lufs, true_peak],
    )?;
    Ok(())
}

/// What normalisation knows about a track's loudness.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrackLoudness {
    /// Integrated loudness, LUFS: measured on the file when there is one, else YouTube's.
    pub lufs: Option<f64>,
    /// True peak, dBTP; only a measurement has it.
    pub true_peak: Option<f64>,
    /// YouTube's value is stored (it is fetched once per track).
    pub has_youtube: bool,
}

pub fn track_loudness(conn: &Connection, id: &TrackId) -> Result<TrackLoudness> {
    Ok(conn
        .query_row(
            "SELECT m.lufs, m.true_peak, t.loudness_lufs FROM tracks t
             LEFT JOIN media_files m ON m.track_id = t.id WHERE t.id = ?1",
            [id.as_str()],
            |r| {
                let (measured, peak, yt): (Option<f64>, Option<f64>, Option<f64>) = (r.get(0)?, r.get(1)?, r.get(2)?);
                Ok(TrackLoudness {
                    lufs: measured.or(yt),
                    true_peak: measured.and(peak),
                    has_youtube: yt.is_some(),
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

/// Stores YouTube's loudness of a track (LUFS).
pub fn set_track_loudness(conn: &Connection, id: &TrackId, lufs: f64) -> Result<()> {
    conn.execute("UPDATE tracks SET loudness_lufs = ?2 WHERE id = ?1", params![id.as_str(), lufs])?;
    Ok(())
}

/// Counts a play; `full` marks a complete listen (data-model.md, play_stats).
pub fn record_play(conn: &Connection, id: &TrackId, full: bool) -> Result<u32> {
    conn.execute(
        "INSERT INTO play_stats(track_id, play_count, full_play_count, last_played_at) VALUES (?1, 1, ?2, ?3)
         ON CONFLICT(track_id) DO UPDATE SET play_count = play_count + 1,
            full_play_count = full_play_count + ?2, last_played_at = ?3",
        params![id.as_str(), full as i64, now_ms()],
    )?;
    Ok(conn.query_row(
        "SELECT full_play_count FROM play_stats WHERE track_id = ?1",
        [id.as_str()],
        |r| r.get(0),
    )?)
}

// ---------- reads ----------

fn placeholders(n: usize) -> String {
    let mut s = String::with_capacity(n * 2);
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s
}

/// Loads tracks by id, preserving the order of `ids` and skipping unknown ones.
pub fn tracks_by_ids(conn: &Connection, ids: &[TrackId]) -> Result<Vec<Track>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut out: HashMap<String, Track> = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        let ph = placeholders(chunk.len());
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT t.id, t.title, t.album_id, a.title, t.track_no, t.duration_ms,
                    COALESCE(t.cover_key, a.cover_key), t.state, t.liked
             FROM tracks t LEFT JOIN albums a ON a.id = t.album_id
             WHERE t.id IN ({ph})"
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter().map(|i| i.as_str())), |r| {
            let album_id: Option<String> = r.get(2)?;
            let album_title: Option<String> = r.get(3)?;
            Ok(Track {
                id: TrackId(r.get(0)?),
                title: r.get(1)?,
                artists: Vec::new(),
                album: album_id.map(|id| AlbumRef { id: AlbumId(id), title: album_title.unwrap_or_default() }),
                track_no: r.get(4)?,
                duration_ms: r.get::<_, i64>(5)? as u64,
                cover_key: r.get(6)?,
                state: TrackState::parse(&r.get::<_, String>(7)?).unwrap_or(TrackState::Remote),
                liked: r.get(8)?,
            })
        })?;
        for t in rows {
            let t = t?;
            out.insert(t.id.0.clone(), t);
        }
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT track_id, artist_id, name FROM track_artists WHERE track_id IN ({ph}) ORDER BY track_id, position"
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter().map(|i| i.as_str())), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?))
        })?;
        for row in rows {
            let (tid, aid, name) = row?;
            if let Some(t) = out.get_mut(&tid) {
                t.artists.push(ArtistRef { id: aid.map(ArtistId), name });
            }
        }
    }
    Ok(ids.iter().filter_map(|id| out.remove(id.as_str())).collect())
}

pub fn track(conn: &Connection, id: &TrackId) -> Result<Option<Track>> {
    Ok(tracks_by_ids(conn, std::slice::from_ref(id))?.pop())
}

/// The album of a track; `None` for an unknown track or one without an album.
pub fn track_album_id(conn: &Connection, id: &TrackId) -> Result<Option<AlbumId>> {
    let album: Option<Option<String>> = conn
        .prepare_cached("SELECT album_id FROM tracks WHERE id = ?1")?
        .query_row([id.as_str()], |r| r.get(0))
        .optional()?;
    Ok(album.flatten().map(AlbumId))
}

fn album_artists(conn: &Connection, ids: &[String]) -> Result<HashMap<String, Vec<ArtistRef>>> {
    let mut out: HashMap<String, Vec<ArtistRef>> = HashMap::new();
    for chunk in ids.chunks(500) {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT album_id, artist_id, name FROM album_artists WHERE album_id IN ({}) ORDER BY album_id, position",
            placeholders(chunk.len())
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?))
        })?;
        for row in rows {
            let (alb, aid, name) = row?;
            out.entry(alb).or_default().push(ArtistRef { id: aid.map(ArtistId), name });
        }
    }
    Ok(out)
}

pub fn albums_by_ids(conn: &Connection, ids: &[AlbumId]) -> Result<Vec<Album>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<String> = ids.iter().map(|i| i.0.clone()).collect();
    let mut artists = album_artists(conn, &keys)?;
    let mut out: HashMap<String, Album> = HashMap::new();
    for chunk in keys.chunks(500) {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT id, title, year, album_type, cover_key, palette, track_count FROM albums WHERE id IN ({})",
            placeholders(chunk.len())
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
            let palette: Option<String> = r.get(5)?;
            Ok(Album {
                id: AlbumId(r.get(0)?),
                title: r.get(1)?,
                artists: Vec::new(),
                year: r.get(2)?,
                album_type: AlbumType::parse(&r.get::<_, String>(3)?),
                cover_key: r.get(4)?,
                palette: palette.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or_default(),
                track_count: r.get(6)?,
            })
        })?;
        for a in rows {
            let mut a = a?;
            a.artists = artists.remove(a.id.as_str()).unwrap_or_default();
            out.insert(a.id.0.clone(), a);
        }
    }
    Ok(ids.iter().filter_map(|id| out.remove(id.as_str())).collect())
}

pub fn album(conn: &Connection, id: &AlbumId) -> Result<Option<Album>> {
    Ok(albums_by_ids(conn, std::slice::from_ref(id))?.pop())
}

pub fn album_track_ids(conn: &Connection, id: &AlbumId) -> Result<Vec<TrackId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM tracks WHERE album_id = ?1 ORDER BY track_no IS NULL, track_no, title",
    )?;
    let ids = stmt.query_map([id.as_str()], |r| Ok(TrackId(r.get(0)?)))?;
    Ok(ids.collect::<std::result::Result<_, _>>()?)
}

/// Whether any of the album's tracks is in the library.
pub fn album_owned(conn: &Connection, id: &AlbumId) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM tracks WHERE album_id = ?1 AND state IN ('downloaded', 'broken'))",
        [id.as_str()],
        |r| r.get(0),
    )?)
}

pub fn artists_by_ids(conn: &Connection, ids: &[ArtistId]) -> Result<Vec<Artist>> {
    let mut out: HashMap<String, Artist> = HashMap::new();
    for chunk in ids.chunks(500) {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT id, name, image_key, in_library, palette FROM artists WHERE id IN ({})",
            placeholders(chunk.len())
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter().map(|i| i.as_str())), |r| {
            let palette: Option<String> = r.get(4)?;
            Ok(Artist {
                id: ArtistId(r.get(0)?),
                name: r.get(1)?,
                image_key: r.get(2)?,
                in_library: r.get(3)?,
                palette: palette.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or_default(),
            })
        })?;
        for a in rows {
            let a = a?;
            out.insert(a.id.0.clone(), a);
        }
    }
    Ok(ids.iter().filter_map(|id| out.remove(id.as_str())).collect())
}

pub fn artist(conn: &Connection, id: &ArtistId) -> Result<Option<Artist>> {
    Ok(artists_by_ids(conn, std::slice::from_ref(id))?.pop())
}

/// Library tracks by this artist, most played first.
pub fn artist_track_ids(conn: &Connection, id: &ArtistId) -> Result<Vec<TrackId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id FROM tracks t
         JOIN track_artists ta ON ta.track_id = t.id AND ta.artist_id = ?1
         LEFT JOIN play_stats p ON p.track_id = t.id
         WHERE t.state IN ('downloaded', 'broken')
         ORDER BY COALESCE(p.play_count, 0) DESC, t.title",
    )?;
    let ids = stmt.query_map([id.as_str()], |r| Ok(TrackId(r.get(0)?)))?;
    Ok(ids.collect::<std::result::Result<_, _>>()?)
}

/// Every known track of the artist, owned or not: newest albums first, album order inside.
pub fn artist_all_track_ids(conn: &Connection, id: &ArtistId) -> Result<Vec<TrackId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.id FROM tracks t
         JOIN track_artists ta ON ta.track_id = t.id AND ta.artist_id = ?1
         LEFT JOIN albums al ON al.id = t.album_id
         GROUP BY t.id
         ORDER BY al.year IS NULL, al.year DESC, al.title, t.track_no IS NULL, t.track_no, t.title",
    )?;
    let ids = stmt.query_map([id.as_str()], |r| Ok(TrackId(r.get(0)?)))?;
    Ok(ids.collect::<std::result::Result<_, _>>()?)
}

/// Albums by this artist that have at least one library track or are saved.
pub fn artist_album_ids(conn: &Connection, id: &ArtistId) -> Result<Vec<AlbumId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT al.id FROM albums al
         JOIN album_artists aa ON aa.album_id = al.id AND aa.artist_id = ?1
         ORDER BY al.year DESC, al.title",
    )?;
    let ids = stmt.query_map([id.as_str()], |r| Ok(AlbumId(r.get(0)?)))?;
    Ok(ids.collect::<std::result::Result<_, _>>()?)
}

pub fn playlists_by_ids(conn: &Connection, ids: &[PlaylistId]) -> Result<Vec<Playlist>> {
    let mut out: HashMap<String, Playlist> = HashMap::new();
    for chunk in ids.chunks(500) {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT p.id, p.title, p.origin,
                    (SELECT count(*) FROM playlist_items i WHERE i.playlist_id = p.id),
                    COALESCE(p.cover_key, (SELECT COALESCE(t.cover_key, a.cover_key) FROM playlist_items i
                        JOIN tracks t ON t.id = i.track_id LEFT JOIN albums a ON a.id = t.album_id
                        WHERE i.playlist_id = p.id ORDER BY i.position LIMIT 1))
             FROM playlists p WHERE p.id IN ({})",
            placeholders(chunk.len())
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk.iter().map(|i| i.as_str())), |r| {
            Ok(Playlist {
                id: PlaylistId(r.get(0)?),
                title: r.get(1)?,
                origin: if r.get::<_, String>(2)? == "ytm" { PlaylistOrigin::Ytm } else { PlaylistOrigin::Local },
                track_count: r.get(3)?,
                cover_key: r.get(4)?,
            })
        })?;
        for p in rows {
            let p = p?;
            out.insert(p.id.0.clone(), p);
        }
    }
    Ok(ids.iter().filter_map(|id| out.remove(id.as_str())).collect())
}

pub fn playlist(conn: &Connection, id: &PlaylistId) -> Result<Option<Playlist>> {
    Ok(playlists_by_ids(conn, std::slice::from_ref(id))?.pop())
}

pub fn playlist_track_ids(conn: &Connection, id: &PlaylistId) -> Result<Vec<TrackId>> {
    let mut stmt =
        conn.prepare_cached("SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position")?;
    let ids = stmt.query_map([id.as_str()], |r| Ok(TrackId(r.get(0)?)))?;
    Ok(ids.collect::<std::result::Result<_, _>>()?)
}

pub fn ensure_found<T>(v: Option<T>, what: &str) -> Result<T> {
    v.ok_or_else(|| Error::Other(mixtape_core::trf!("{what} not found", "{what}: не знайдено", "{what} не найден")))
}

// ---------- home ----------

fn ids_query<T>(conn: &Connection, sql: &str, limit: usize, wrap: impl Fn(String) -> T) -> Result<Vec<T>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([limit as i64], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(wrap(r?));
    }
    Ok(out)
}

/// Albums with the most recently played tracks first.
pub fn recently_played_album_ids(conn: &Connection, limit: usize) -> Result<Vec<AlbumId>> {
    ids_query(
        conn,
        "SELECT t.album_id FROM play_stats p JOIN tracks t ON t.id = p.track_id
         WHERE t.album_id IS NOT NULL GROUP BY t.album_id ORDER BY MAX(p.last_played_at) DESC LIMIT ?1",
        limit,
        AlbumId,
    )
}

/// Albums whose tracks were added to the library most recently.
pub fn recently_added_album_ids(conn: &Connection, limit: usize) -> Result<Vec<AlbumId>> {
    ids_query(
        conn,
        "SELECT album_id FROM tracks WHERE album_id IS NOT NULL AND state IN ('downloaded', 'broken')
         GROUP BY album_id ORDER BY MAX(added_at) DESC LIMIT ?1",
        limit,
        AlbumId,
    )
}

/// Library tracks played most often.
pub fn most_played_track_ids(conn: &Connection, limit: usize) -> Result<Vec<TrackId>> {
    ids_query(
        conn,
        "SELECT p.track_id FROM play_stats p JOIN tracks t ON t.id = p.track_id
         WHERE t.state IN ('downloaded', 'broken') AND p.play_count > 0
         ORDER BY p.play_count DESC, p.last_played_at DESC LIMIT ?1",
        limit,
        TrackId,
    )
}

pub fn liked_count(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT count(*) FROM tracks WHERE liked = 1", [], |r| r.get(0))?)
}

/// Library tracks by the artists of `seeds` in random order, then other library tracks:
/// what a mix plays when YouTube Music is out of reach.
pub fn offline_mix_ids(conn: &Connection, seeds: &[TrackId], limit: usize) -> Result<Vec<TrackId>> {
    let seeds_json = serde_json::to_string(&seeds.iter().map(|t| t.as_str()).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".into());
    let mut stmt = conn.prepare_cached(
        "WITH seed_artists AS (
             SELECT DISTINCT ta.name FROM track_artists ta
             WHERE ta.track_id IN (SELECT value FROM json_each(?1))
         )
         SELECT t.id FROM tracks t
         WHERE t.state = 'downloaded' AND t.id NOT IN (SELECT value FROM json_each(?1))
         ORDER BY EXISTS (
             SELECT 1 FROM track_artists ta WHERE ta.track_id = t.id AND ta.name IN (SELECT name FROM seed_artists)
         ) DESC, random()
         LIMIT ?2",
    )?;
    let ids = stmt.query_map(params![seeds_json, limit as i64], |r| Ok(TrackId(r.get(0)?)))?;
    Ok(ids.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::Db;

    pub fn meta(id: &str, title: &str, artist: (&str, &str), album: Option<(&str, &str)>) -> TrackMeta {
        TrackMeta {
            id: TrackId::from(id),
            title: title.into(),
            artists: vec![ArtistRef { id: Some(ArtistId::from(artist.0)), name: artist.1.into() }],
            album: album.map(|(i, t)| AlbumRef { id: AlbumId::from(i), title: t.into() }),
            track_no: None,
            duration_ms: 180_000,
            cover_key: None,
        }
    }

    pub fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn offline_mix_prefers_seed_artists_and_skips_queued() {
        let (_d, db) = db();
        db.write(|t| {
            for (id, artist) in [("s", ("a1", "A")), ("x", ("a1", "A")), ("y", ("a2", "B")), ("r", ("a1", "A"))] {
                upsert_track(t, &meta(id, id, artist, None))?;
                if id != "r" {
                    set_state(t, &TrackId::from(id), TrackState::Downloaded)?;
                }
            }
            Ok(())
        })
        .unwrap();
        let ids = db.read(|c| offline_mix_ids(c, &[TrackId::from("s")], 10)).unwrap();
        let names: Vec<&str> = ids.iter().map(|t| t.as_str()).collect();
        assert_eq!(names, ["x", "y"], "same artist first, no seed, no remote tracks");
    }

    #[test]
    fn upsert_and_load_preserves_order_and_artists() {
        let (_d, db) = db();
        db.write(|t| {
            upsert_track(t, &meta("a", "Take on Me", ("UCaha", "a-ha"), Some(("MPRE1", "Hunting High"))))?;
            upsert_track(t, &meta("b", "Kokomo", ("UCbb", "The Beach Boys"), None))?;
            set_state(t, &TrackId::from("a"), TrackState::Downloaded)?;
            set_liked(t, &TrackId::from("b"), true)?;
            Ok(())
        })
        .unwrap();
        let ts = db.read(|c| tracks_by_ids(c, &[TrackId::from("b"), TrackId::from("x"), TrackId::from("a")])).unwrap();
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].title, "Kokomo");
        assert!(ts[0].liked);
        assert_eq!(ts[1].artists[0].name, "a-ha");
        assert_eq!(ts[1].album.as_ref().unwrap().title, "Hunting High");
        assert_eq!(ts[1].state, TrackState::Downloaded);
        // Re-upserting metadata keeps the state.
        db.write(|t| upsert_track(t, &meta("a", "Take On Me", ("UCaha", "a-ha"), None))).unwrap();
        let a = db.read(|c| track(c, &TrackId::from("a"))).unwrap().unwrap();
        assert_eq!(a.state, TrackState::Downloaded);
        assert_eq!(a.title, "Take On Me");
        assert!(a.album.is_some());
        let album = |id: &str| db.read(|c| track_album_id(c, &TrackId::from(id))).unwrap();
        assert_eq!(album("a"), Some(AlbumId::from("MPRE1")));
        assert_eq!(album("b"), None);
        assert_eq!(album("x"), None);
    }

    #[test]
    fn media_file_moves_state_and_counts_plays() {
        let (_d, db) = db();
        let id = TrackId::from("a");
        db.write(move |t| {
            upsert_track(t, &meta("a", "A", ("UC1", "X"), None))?;
            put_media_file(
                t,
                &MediaFile {
                    track_id: TrackId::from("a"),
                    path: "library/a.webm".into(),
                    container: "webm".into(),
                    itag: Some(251),
                    size_bytes: 10,
                    sample_rate: Some(48_000),
                    location: FileLocation::Library,
                },
            )?;
            assert_eq!(record_play(t, &TrackId::from("a"), true)?, 1);
            assert_eq!(record_play(t, &TrackId::from("a"), false)?, 1);
            Ok(())
        })
        .unwrap();
        let t = db.read(|c| track(c, &id)).unwrap().unwrap();
        assert_eq!(t.state, TrackState::Downloaded);
        let m = db.read(|c| media_file(c, &id)).unwrap().unwrap();
        assert_eq!(m.location, FileLocation::Library);
    }

    #[test]
    fn loudness_prefers_the_measurement() {
        let (_d, db) = db();
        let id = TrackId::from("a");
        let get = |db: &Db| db.read(|c| track_loudness(c, &TrackId::from("a"))).unwrap();
        db.write(|t| upsert_track(t, &meta("a", "A", ("UC1", "X"), None))).unwrap();
        assert_eq!(get(&db), TrackLoudness::default());
        db.write(|t| set_track_loudness(t, &TrackId::from("a"), -8.86)).unwrap();
        assert_eq!(get(&db), TrackLoudness { lufs: Some(-8.86), true_peak: None, has_youtube: true });
        db.write(move |t| {
            put_media_file(
                t,
                &MediaFile {
                    track_id: id.clone(),
                    path: "cache/a.webm".into(),
                    container: "webm".into(),
                    itag: Some(251),
                    size_bytes: 10,
                    sample_rate: Some(48_000),
                    location: FileLocation::Cache,
                },
            )?;
            // Not measured yet: YouTube's value still counts.
            assert_eq!(track_loudness(t, &id)?.lufs, Some(-8.86));
            set_loudness(t, &id, -9.5, -0.3)
        })
        .unwrap();
        assert_eq!(get(&db), TrackLoudness { lufs: Some(-9.5), true_peak: Some(-0.3), has_youtube: true });
        assert_eq!(db.read(|c| track_loudness(c, &TrackId::from("x"))).unwrap(), TrackLoudness::default());
    }
}
