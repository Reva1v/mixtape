//! Listening statistics: the play log (`play_events`) and the recap built from it.
//!
//! Hours, weekdays and days are local time, given as a UTC offset by the caller, so the
//! queries do not depend on the machine's time zone settings.

use crate::Result;
use crate::repo::{albums_by_ids, tracks_by_ids};
use mixtape_core::{Album, AlbumId, ArtistId, Track, TrackId};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
/// Rows in each top list.
const TOP: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StatsPeriod {
    /// The last 7 × 24 hours.
    Week,
    /// The last 30 × 24 hours.
    Month,
    /// Since 1 January, local time.
    Year,
    All,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TopTrack {
    pub track: Track,
    pub plays: u32,
    #[ts(type = "number")]
    pub listened_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TopArtist {
    /// `None` for a name without a YouTube Music artist behind it.
    #[ts(optional)]
    pub id: Option<ArtistId>,
    pub name: String,
    #[ts(optional)]
    pub image_key: Option<String>,
    pub plays: u32,
    #[ts(type = "number")]
    pub listened_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TopAlbum {
    pub album: Album,
    pub plays: u32,
    #[ts(type = "number")]
    pub listened_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TopDay {
    /// Local midnight that starts the day, as Unix ms.
    #[ts(type = "number")]
    pub day_ms: i64,
    pub plays: u32,
    #[ts(type = "number")]
    pub listened_ms: u64,
}

/// One device's share of a period (phone ↔ PC sync, specs 007).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceTotal {
    /// The device's id (`play_events.device`); what [`listening_for`] filters by.
    pub device: String,
    /// This device's name, or the paired device's; `None` for a device no longer known.
    #[ts(optional)]
    pub name: Option<String>,
    /// The device this database belongs to.
    pub this_device: bool,
    #[ts(type = "number")]
    pub listened_ms: u64,
    pub plays: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListeningStats {
    #[ts(type = "number")]
    pub listened_ms: u64,
    pub plays: u32,
    /// Distinct tracks and artists played.
    pub tracks: u32,
    pub artists: u32,
    pub top_tracks: Vec<TopTrack>,
    pub top_artists: Vec<TopArtist>,
    pub top_albums: Vec<TopAlbum>,
    /// Listening time by local hour, 0–23.
    #[ts(type = "number[]")]
    pub by_hour: Vec<u64>,
    /// Listening time by weekday, Monday first.
    #[ts(type = "number[]")]
    pub by_weekday: Vec<u64>,
    #[ts(optional)]
    pub top_day: Option<TopDay>,
    /// The period by device, most listened first. Always over all devices, also when the
    /// rest is filtered to one.
    pub by_device: Vec<DeviceTotal>,
}

/// Logs one play of this device. Its global id is `(settings.device_id, id)`: `seq` is the
/// id the row is about to get, the next AUTOINCREMENT value after `sqlite_sequence`.
pub fn record(conn: &Connection, id: &TrackId, started_at: i64, listened_ms: u64) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO play_events(track_id, started_at, listened_ms, device, seq)
         VALUES (?1, ?2, ?3,
            (SELECT json_extract(value, '$') FROM settings WHERE key = 'device_id'),
            COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'play_events'), 0) + 1)",
    )?
    .execute(params![id.as_str(), started_at, listened_ms as i64])?;
    Ok(())
}

/// First moment of `period` (Unix ms) as of `now`; `offset_ms` is local time minus UTC.
pub fn period_start(
    conn: &Connection,
    period: StatsPeriod,
    now: i64,
    offset_ms: i64,
) -> Result<i64> {
    Ok(match period {
        StatsPeriod::Week => now - 7 * DAY_MS,
        StatsPeriod::Month => now - 30 * DAY_MS,
        StatsPeriod::Year => {
            let local_secs: i64 = conn.query_row(
                "SELECT CAST(strftime('%s', ?1, 'unixepoch', 'start of year') AS INTEGER)",
                [(now + offset_ms).div_euclid(1000)],
                |r| r.get(0),
            )?;
            local_secs * 1000 - offset_ms
        }
        StatsPeriod::All => i64::MIN,
    })
}

/// The recap of everything played since `since` (Unix ms), on every device.
pub fn listening(conn: &Connection, since: i64, offset_ms: i64) -> Result<ListeningStats> {
    listening_for(conn, since, offset_ms, None)
}

/// [`listening`] of the plays logged on `device` only (`None`: every device).
/// [`ListeningStats::by_device`] still covers every device.
pub fn listening_for(
    conn: &Connection,
    since: i64,
    offset_ms: i64,
    device: Option<&str>,
) -> Result<ListeningStats> {
    let mut s = ListeningStats {
        by_hour: vec![0; 24],
        by_weekday: vec![0; 7],
        by_device: by_device(conn, since)?,
        ..Default::default()
    };

    // One pass over the period (the covering index) for totals, hours, weekdays, days
    // and tracks.
    let mut days: HashMap<i64, (u64, u32)> = HashMap::new();
    let mut per: HashMap<String, (u32, u64)> = HashMap::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT started_at, listened_ms, track_id FROM play_events
             WHERE started_at >= ?1 AND (?2 IS NULL OR device = ?2)",
        )?;
        let mut rows = stmt.query(params![since, device])?;
        while let Some(r) = rows.next()? {
            let (at, ms): (i64, i64) = (r.get(0)?, r.get(1)?);
            let ms = ms.max(0) as u64;
            let local = at + offset_ms;
            let day = local.div_euclid(DAY_MS);
            s.plays += 1;
            s.listened_ms += ms;
            s.by_hour[(local.rem_euclid(DAY_MS) / HOUR_MS) as usize] += ms;
            // 1970-01-01 was a Thursday: day 0 is weekday 3 counting from Monday.
            s.by_weekday[(day + 3).rem_euclid(7) as usize] += ms;
            let d = days.entry(day).or_default();
            d.0 += ms;
            d.1 += 1;
            let id = r.get_ref(2)?.as_str().map_err(rusqlite::Error::from)?;
            let t = match per.get_mut(id) {
                Some(t) => t,
                None => per.entry(id.to_owned()).or_default(),
            };
            t.0 += 1;
            t.1 += ms;
        }
    }
    if s.plays == 0 {
        return Ok(s);
    }
    s.top_day = days
        .into_iter()
        .max_by_key(|&(day, (ms, plays))| (ms, plays, -day))
        .map(|(day, (listened_ms, plays))| TopDay {
            day_ms: day * DAY_MS - offset_ms,
            plays,
            listened_ms,
        });

    // Tracks.
    s.tracks = per.len() as u32;
    let mut counts: Vec<(&str, u32, u64)> = per
        .iter()
        .map(|(id, &(plays, ms))| (id.as_str(), plays, ms))
        .collect();
    counts.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(b.0)));
    let ids: Vec<TrackId> = counts
        .iter()
        .take(TOP)
        .map(|c| TrackId::from(c.0))
        .collect();
    let mut tracks: HashMap<String, Track> = tracks_by_ids(conn, &ids)?
        .into_iter()
        .map(|t| (t.id.0.clone(), t))
        .collect();
    s.top_tracks = counts
        .iter()
        .take(TOP)
        .filter_map(|&(id, plays, listened_ms)| {
            tracks.remove(id).map(|track| TopTrack {
                track,
                plays,
                listened_ms,
            })
        })
        .collect();

    // Artists and albums of the played tracks. A play counts for every credited artist;
    // names without an artist id are grouped by name.
    let mut by_artist: HashMap<String, TopArtist> = HashMap::new();
    let mut by_album: HashMap<String, (u32, u64)> = HashMap::new();
    let mut album_of: HashMap<String, Option<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT t.id, t.album_id, ta.artist_id, COALESCE(a.name, ta.name), a.image_key
             FROM tracks t
             LEFT JOIN track_artists ta ON ta.track_id = t.id
             LEFT JOIN artists a ON a.id = ta.artist_id
             WHERE t.id IN (SELECT track_id FROM play_events
                            WHERE started_at >= ?1 AND (?2 IS NULL OR device = ?2))",
        )?;
        let mut rows = stmt.query(params![since, device])?;
        while let Some(r) = rows.next()? {
            let track: String = r.get(0)?;
            let Some(&(plays, ms)) = per.get(&track) else {
                continue;
            };
            let artist_id: Option<String> = r.get(2)?;
            if let Some(name) = r.get::<_, Option<String>>(3)? {
                let key = artist_id.clone().unwrap_or_else(|| format!("\u{1}{name}"));
                let a = by_artist.entry(key).or_insert_with(|| TopArtist {
                    id: artist_id.map(ArtistId),
                    name,
                    image_key: None,
                    plays: 0,
                    listened_ms: 0,
                });
                a.plays += plays;
                a.listened_ms += ms;
                if a.image_key.is_none() {
                    a.image_key = r.get(4)?;
                }
            }
            if let std::collections::hash_map::Entry::Vacant(e) = album_of.entry(track) {
                e.insert(r.get(1)?);
            }
        }
    }
    for (track, album) in album_of {
        if let (Some(album), Some(&(plays, ms))) = (album, per.get(&track)) {
            let e = by_album.entry(album).or_default();
            e.0 += plays;
            e.1 += ms;
        }
    }
    s.artists = by_artist.len() as u32;
    let mut artists: Vec<TopArtist> = by_artist.into_values().collect();
    artists.sort_unstable_by(|a, b| {
        b.plays
            .cmp(&a.plays)
            .then(b.listened_ms.cmp(&a.listened_ms))
            .then_with(|| a.name.cmp(&b.name))
    });
    artists.truncate(TOP);
    s.top_artists = artists;

    // Albums.
    let mut albums: Vec<(AlbumId, u32, u64)> = by_album
        .into_iter()
        .map(|(id, (plays, ms))| (AlbumId(id), plays, ms))
        .collect();
    albums.sort_unstable_by(|a, b| {
        b.1.cmp(&a.1)
            .then(b.2.cmp(&a.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    albums.truncate(TOP);
    let ids: Vec<AlbumId> = albums.iter().map(|a| a.0.clone()).collect();
    let mut found: HashMap<AlbumId, Album> = albums_by_ids(conn, &ids)?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();
    s.top_albums = albums
        .into_iter()
        .filter_map(|(id, plays, listened_ms)| {
            found.remove(&id).map(|album| TopAlbum {
                album,
                plays,
                listened_ms,
            })
        })
        .collect();
    Ok(s)
}

/// Plays and time since `since` per device, named from the settings and the pairing.
fn by_device(conn: &Connection, since: i64) -> Result<Vec<DeviceTotal>> {
    let me = crate::settings::device_id(conn)?;
    let peer = crate::peer_store::pairing(conn)?;
    let mut stmt = conn.prepare_cached(
        "SELECT device, COALESCE(SUM(MAX(listened_ms, 0)), 0), COUNT(*) FROM play_events
         WHERE started_at >= ?1 GROUP BY device",
    )?;
    let rows = stmt.query_map([since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (device, ms, plays) = row?;
        let this_device = device == me;
        let name = if this_device {
            Some(crate::settings::device_name(conn)?)
        } else {
            peer.as_ref()
                .filter(|p| p.peer_device == device)
                .map(|p| p.peer_name.clone())
        };
        out.push(DeviceTotal {
            device,
            name,
            this_device,
            listened_ms: ms as u64,
            plays: plays as u32,
        });
    }
    out.sort_unstable_by(|a, b| {
        b.listened_ms
            .cmp(&a.listened_ms)
            .then_with(|| a.device.cmp(&b.device))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::upsert_track;
    use std::time::Instant;

    const MIN: u64 = 60_000;
    /// Monday 2026-10-12 00:00 UTC.
    const MONDAY: i64 = 1_791_763_200_000;

    fn seed(conn: &Connection) -> Result<()> {
        upsert_track(
            conn,
            &meta("a", "Alpha", ("UCx", "X"), Some(("MPRE1", "One"))),
        )?;
        upsert_track(
            conn,
            &meta("b", "Beta", ("UCx", "X"), Some(("MPRE1", "One"))),
        )?;
        let mut c = meta("c", "Gamma", ("UCy", "Y"), Some(("MPRE2", "Two")));
        // A duet: counts for both artists; the second has no id.
        c.artists.push(mixtape_core::ArtistRef {
            id: None,
            name: "Z".into(),
        });
        upsert_track(conn, &c)?;
        conn.execute(
            "UPDATE artists SET image_key = 'pic-x' WHERE id = 'UCx'",
            [],
        )?;
        Ok(())
    }

    #[test]
    fn monday_is_a_monday() {
        // 2026-10-12 is a Monday: (days since epoch + 3) % 7 == 0.
        assert_eq!((MONDAY / DAY_MS + 3) % 7, 0);
    }

    #[test]
    fn recap_counts_tops_hours_and_days() {
        let (_d, db) = db();
        db.write(|t| {
            seed(t)?;
            let id = |s: &str| TrackId::from(s);
            // Monday 09:xx UTC: a, a, c.
            record(t, &id("a"), MONDAY + 9 * HOUR_MS, 3 * MIN)?;
            record(t, &id("a"), MONDAY + 9 * HOUR_MS + 5 * MIN as i64, 3 * MIN)?;
            record(t, &id("c"), MONDAY + 9 * HOUR_MS + 10 * MIN as i64, 4 * MIN)?;
            // Tuesday 23:30 UTC: b.
            record(
                t,
                &id("b"),
                MONDAY + DAY_MS + 23 * HOUR_MS + 30 * MIN as i64,
                2 * MIN,
            )?;
            // Long ago: outside a recent period.
            record(t, &id("c"), MONDAY - 400 * DAY_MS, 5 * MIN)?;
            Ok(())
        })
        .unwrap();

        let since = MONDAY - DAY_MS;
        let s = db.read(|c| listening(c, since, 0)).unwrap();
        assert_eq!(
            (s.plays, s.listened_ms, s.tracks, s.artists),
            (4, 12 * MIN, 3, 3)
        );
        let top: Vec<(&str, u32)> = s
            .top_tracks
            .iter()
            .map(|t| (t.track.id.as_str(), t.plays))
            .collect();
        assert_eq!(
            top,
            [("a", 2), ("c", 1), ("b", 1)],
            "plays first, then time"
        );
        assert_eq!(s.top_tracks[0].track.title, "Alpha");
        let artists: Vec<(Option<&str>, &str, u32)> = s
            .top_artists
            .iter()
            .map(|a| (a.id.as_ref().map(|i| i.as_str()), a.name.as_str(), a.plays))
            .collect();
        assert_eq!(
            artists,
            [(Some("UCx"), "X", 3), (Some("UCy"), "Y", 1), (None, "Z", 1)]
        );
        assert_eq!(s.top_artists[0].image_key.as_deref(), Some("pic-x"));
        let albums: Vec<(&str, u32, u64)> = s
            .top_albums
            .iter()
            .map(|a| (a.album.id.as_str(), a.plays, a.listened_ms))
            .collect();
        assert_eq!(albums, [("MPRE1", 3, 8 * MIN), ("MPRE2", 1, 4 * MIN)]);
        assert_eq!(s.by_hour[9], 10 * MIN);
        assert_eq!(s.by_hour[23], 2 * MIN);
        assert_eq!(s.by_weekday[0], 10 * MIN, "Monday");
        assert_eq!(s.by_weekday[1], 2 * MIN, "Tuesday");
        assert_eq!(
            s.top_day,
            Some(TopDay {
                day_ms: MONDAY,
                plays: 3,
                listened_ms: 10 * MIN
            })
        );

        // At UTC+3 Tuesday's 23:30 is Wednesday 02:30.
        let off = 3 * HOUR_MS;
        let s = db.read(|c| listening(c, since, off)).unwrap();
        assert_eq!((s.by_hour[12], s.by_hour[2]), (10 * MIN, 2 * MIN));
        assert_eq!((s.by_weekday[1], s.by_weekday[2]), (0, 2 * MIN));
        assert_eq!(s.top_day.unwrap().day_ms, MONDAY - off);

        // All time includes the old play.
        let all = db.read(|c| listening(c, i64::MIN, 0)).unwrap();
        assert_eq!((all.plays, all.listened_ms), (5, 17 * MIN));
        assert_eq!(
            all.top_tracks[0].track.id.as_str(),
            "c",
            "c: 2 plays, 9 min"
        );
    }

    #[test]
    fn filters_by_device_and_splits_by_device() {
        use crate::peer_store::{Pairing, PeerEvent, PeerTrack, merge_events, save_pairing};
        let (_d, db) = db();
        db.write(|t| {
            seed(t)?;
            record(t, &TrackId::from("a"), MONDAY, 3 * MIN)?;
            record(t, &TrackId::from("b"), MONDAY + HOUR_MS, 2 * MIN)?;
            let ev = |seq, track: &str, ms| PeerEvent {
                device: "phone".into(),
                seq,
                started_at: MONDAY + 2 * HOUR_MS,
                listened_ms: ms,
                track: PeerTrack {
                    id: TrackId::from(track),
                    title: track.into(),
                    artists: Vec::new(),
                    album: None,
                    duration_ms: 0,
                },
            };
            merge_events(t, &[ev(1, "c", 4 * MIN), ev(2, "c", 4 * MIN), ev(3, "a", MIN)])?;
            t.execute(
                "INSERT INTO settings(key, value) VALUES ('device_name', '\"Desktop\"')",
                [],
            )?;
            save_pairing(
                t,
                &Pairing {
                    peer_device: "phone".into(),
                    peer_name: "Galaxy".into(),
                    key: [0; 32],
                    addresses: Vec::new(),
                    last_address: None,
                    paired_at: 0,
                    last_exchange: None,
                },
            )
        })
        .unwrap();
        let me = db.read(crate::settings::device_id).unwrap();

        let all = db.read(|c| listening(c, i64::MIN, 0)).unwrap();
        assert_eq!((all.plays, all.listened_ms), (5, 14 * MIN));
        let split: Vec<(&str, Option<&str>, bool, u64, u32)> = all
            .by_device
            .iter()
            .map(|d| (d.device.as_str(), d.name.as_deref(), d.this_device, d.listened_ms, d.plays))
            .collect();
        assert_eq!(
            split,
            [
                ("phone", Some("Galaxy"), false, 9 * MIN, 3),
                (me.as_str(), Some("Desktop"), true, 5 * MIN, 2)
            ]
        );

        let phone = db.read(|c| listening_for(c, i64::MIN, 0, Some("phone"))).unwrap();
        assert_eq!((phone.plays, phone.listened_ms, phone.tracks), (3, 9 * MIN, 2));
        assert_eq!(phone.top_tracks[0].track.id.as_str(), "c");
        assert_eq!(phone.by_hour[2], 9 * MIN);
        assert_eq!(phone.by_device, all.by_device, "the split ignores the filter");
        let artists: Vec<&str> = phone.top_artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(artists, ["Y", "Z", "X"], "c twice, a once");

        let mine = db.read(|c| listening_for(c, i64::MIN, 0, Some(&me))).unwrap();
        assert_eq!((mine.plays, mine.listened_ms), (2, 5 * MIN));
        let none = db.read(|c| listening_for(c, i64::MIN, 0, Some("tablet"))).unwrap();
        assert_eq!((none.plays, none.by_device.len()), (0, 2));
    }

    #[test]
    fn empty_period() {
        let (_d, db) = db();
        let s = db.read(|c| listening(c, 0, 0)).unwrap();
        assert_eq!(s.plays, 0);
        assert!(s.top_tracks.is_empty() && s.top_day.is_none());
        assert_eq!((s.by_hour.len(), s.by_weekday.len()), (24, 7));
    }

    #[test]
    fn period_starts() {
        let (_d, db) = db();
        let now = MONDAY + 10 * HOUR_MS;
        let start = |p, off| db.read(|c| period_start(c, p, now, off)).unwrap();
        assert_eq!(start(StatsPeriod::Week, 0), now - 7 * DAY_MS);
        assert_eq!(start(StatsPeriod::Month, 0), now - 30 * DAY_MS);
        assert_eq!(start(StatsPeriod::All, 0), i64::MIN);
        // 2026-01-01 00:00 UTC.
        assert_eq!(start(StatsPeriod::Year, 0), 1_767_225_600_000);
        // Local midnight at UTC+2 is 22:00 UTC the day before.
        assert_eq!(
            start(StatsPeriod::Year, 2 * HOUR_MS),
            1_767_225_600_000 - 2 * HOUR_MS
        );
    }

    #[test]
    fn deleting_a_track_drops_its_events() {
        let (_d, db) = db();
        db.write(|t| {
            seed(t)?;
            record(t, &TrackId::from("a"), MONDAY, MIN)?;
            t.execute("DELETE FROM tracks WHERE id = 'a'", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.read(|c| listening(c, i64::MIN, 0)).unwrap().plays, 0);
    }

    /// 20k tracks and 100k events: the whole recap stays interactive.
    #[test]
    fn fast_on_a_big_library() {
        let (_d, db) = db();
        db.write(|t| {
            let mut track = t.prepare("INSERT INTO tracks(id, title, album_id) VALUES (?1, ?1, ?2)")?;
            let mut ta = t.prepare("INSERT INTO track_artists(track_id, position, artist_id, name) VALUES (?1, 0, ?2, ?2)")?;
            let mut album = t.prepare("INSERT INTO albums(id, title) VALUES (?1, ?1)")?;
            let mut artist = t.prepare("INSERT INTO artists(id, name) VALUES (?1, ?1)")?;
            for i in 0..2_000 {
                album.execute([format!("al{i}")])?;
            }
            for i in 0..500 {
                artist.execute([format!("ar{i}")])?;
            }
            for i in 0..20_000 {
                let id = format!("t{i}");
                track.execute(params![id, format!("al{}", i / 10)])?;
                ta.execute(params![id, format!("ar{}", i % 500)])?;
            }
            let mut rnd = 7u64;
            for i in 0..100_000i64 {
                rnd = rnd.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                let tr = (rnd >> 33) % 20_000;
                record(t, &TrackId(format!("t{tr}")), MONDAY - i * 300_000, 200_000)?;
            }
            Ok(())
        })
        .unwrap();
        let started = Instant::now();
        let s = db.read(|c| listening(c, i64::MIN, 0)).unwrap();
        let took = started.elapsed();
        assert_eq!(s.plays, 100_000);
        assert_eq!(
            (s.top_tracks.len(), s.top_artists.len(), s.top_albums.len()),
            (10, 10, 10)
        );
        eprintln!("recap of 100k events: {took:?}");
        assert!(took.as_millis() < 1_500, "took {took:?}");
    }
}
