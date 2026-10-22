//! The paired device and the play log as the phone ↔ PC exchange sees it (specs 007).
//!
//! A play's global id is `(device, seq)` (migration 009). The log only grows, so each side
//! sums up what it has as cursors, the highest `seq` per device, and the other side sends
//! everything above them. The wire protocol lives in `mixtape_sync::peer`.
//!
//! The pairing is one JSON value in `settings` under [`PEER_KEY`]: one peer per device in this
//! version, read together with the rest of the settings. Its key is stored in the clear like
//! the other secrets in the database; an app that keeps it elsewhere (the Android vault) can
//! save a zeroed key here and put the real one back before an exchange.

use crate::repo::{TrackMeta, tracks_by_ids, upsert_track};
use crate::Result;
use mixtape_core::{AlbumRef, ArtistRef, TrackId};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// The `settings` key of the [`Pairing`].
pub const PEER_KEY: &str = "peer";
/// Events per batch of the exchange.
pub const BATCH: usize = 1000;

/// Highest `seq` known per device id.
pub type Cursors = BTreeMap<String, i64>;

/// The device this one is paired with.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    /// The peer's `settings.device_id`.
    pub peer_device: String,
    pub peer_name: String,
    /// The shared secret both sides derive the session keys from.
    #[serde(with = "hex_key")]
    pub key: [u8; 32],
    /// Where to reach the peer as `host:port`, from the pairing code. Empty on the PC, which
    /// only answers.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// The address the last exchange went through, tried before the others.
    #[serde(default)]
    pub last_address: Option<String>,
    /// Unix ms.
    pub paired_at: i64,
    /// The last finished exchange, Unix ms.
    #[serde(default)]
    pub last_exchange: Option<i64>,
}

impl std::fmt::Debug for Pairing {
    // Without the key: pairings end up in logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing")
            .field("peer_device", &self.peer_device)
            .field("peer_name", &self.peer_name)
            .field("addresses", &self.addresses)
            .field("last_address", &self.last_address)
            .field("paired_at", &self.paired_at)
            .field("last_exchange", &self.last_exchange)
            .finish_non_exhaustive()
    }
}

/// The track of an event, enough to make a stub of it on a device that doesn't have it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerTrack {
    pub id: TrackId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    #[serde(default)]
    pub album: Option<AlbumRef>,
    pub duration_ms: u64,
}

/// One play as it travels between devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerEvent {
    pub device: String,
    pub seq: i64,
    /// Unix ms, by the clock of the device that logged it.
    pub started_at: i64,
    pub listened_ms: u64,
    pub track: PeerTrack,
}

/// The stored pairing, if any.
pub fn pairing(conn: &Connection) -> Result<Option<Pairing>> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [PEER_KEY], |r| r.get(0))
        .optional()?;
    Ok(raw.map(|s| serde_json::from_str(&s)).transpose()?)
}

/// Stores `p`, replacing any earlier pairing.
pub fn save_pairing(conn: &Connection, p: &Pairing) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![PEER_KEY, serde_json::to_string(p)?],
    )?;
    Ok(())
}

/// Forgets the pairing. The plays already exchanged stay.
pub fn remove_pairing(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [PEER_KEY])?;
    Ok(())
}

/// Notes a finished exchange at `at` (Unix ms), through `address` when the caller dialled one.
pub fn note_exchange(conn: &Connection, address: Option<&str>, at: i64) -> Result<()> {
    if let Some(mut p) = pairing(conn)? {
        p.last_exchange = Some(at);
        if let Some(a) = address {
            p.last_address = Some(a.to_owned());
        }
        save_pairing(conn, &p)?;
    }
    Ok(())
}

/// What this log has: the highest `seq` per device.
pub fn cursors(conn: &Connection) -> Result<Cursors> {
    let mut stmt = conn.prepare_cached("SELECT device, MAX(seq) FROM play_events GROUP BY device")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Up to `limit` events the holder of `theirs` lacks, by device, then `seq`. The caller sends
/// them and moves its copy of `theirs` past them with [`advance`] for the next batch.
pub fn events_after(conn: &Connection, theirs: &Cursors, limit: usize) -> Result<Vec<PeerEvent>> {
    let mut raw: Vec<(String, i64, String, i64, i64)> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT seq, track_id, started_at, listened_ms FROM play_events
             WHERE device = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        for (device, &max) in &cursors(conn)? {
            let from = theirs.get(device).copied().unwrap_or(0);
            if max <= from || raw.len() >= limit {
                continue;
            }
            let rows = stmt.query_map(params![device, from, (limit - raw.len()) as i64], |r| {
                Ok((device.clone(), r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            for row in rows {
                raw.push(row?);
            }
        }
    }
    let mut ids: Vec<TrackId> = raw.iter().map(|e| TrackId::from(e.2.as_str())).collect();
    ids.sort_unstable();
    ids.dedup();
    let tracks: HashMap<String, PeerTrack> = tracks_by_ids(conn, &ids)?
        .into_iter()
        .map(|t| {
            let track = PeerTrack {
                id: t.id.clone(),
                title: t.title,
                artists: t.artists,
                album: t.album,
                duration_ms: t.duration_ms,
            };
            (t.id.0, track)
        })
        .collect();
    Ok(raw
        .into_iter()
        .filter_map(|(device, seq, track_id, started_at, listened_ms)| {
            Some(PeerEvent {
                device,
                seq,
                started_at,
                listened_ms: listened_ms.max(0) as u64,
                track: tracks.get(&track_id)?.clone(),
            })
        })
        .collect())
}

/// Moves `cursors` past `events` (sent or received).
pub fn advance(cursors: &mut Cursors, events: &[PeerEvent]) {
    for e in events {
        let c = cursors.entry(e.device.clone()).or_insert(0);
        *c = (*c).max(e.seq);
    }
}

/// Adds the plays this log lacks; returns how many were new. Unknown tracks get a `remote`
/// stub with the event's title, artists and album, so they show up in the statistics. The
/// play counts of `play_stats` grow by the new plays (a play is full at 90 % of the track, as
/// the player counts it). Run it inside a transaction: a batch lands whole or not at all.
pub fn merge_events(conn: &Connection, events: &[PeerEvent]) -> Result<usize> {
    let mut known = conn.prepare_cached("SELECT 1 FROM play_events WHERE device = ?1 AND seq = ?2")?;
    let mut duration =
        conn.prepare_cached("SELECT duration_ms FROM tracks WHERE id = ?1")?;
    let mut insert = conn.prepare_cached(
        "INSERT OR IGNORE INTO play_events(track_id, started_at, listened_ms, device, seq)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    // Per track: plays, full plays, last start.
    let mut added: HashMap<&str, (i64, i64, i64)> = HashMap::new();
    let mut n = 0;
    for e in events {
        if e.device.is_empty() || known.exists(params![e.device, e.seq])? {
            continue;
        }
        let id = e.track.id.as_str();
        let dur: i64 = match duration.query_row([id], |r| r.get(0)).optional()? {
            Some(d) => d,
            None => {
                upsert_track(
                    conn,
                    &TrackMeta {
                        id: e.track.id.clone(),
                        title: e.track.title.clone(),
                        artists: e.track.artists.clone(),
                        album: e.track.album.clone(),
                        track_no: None,
                        duration_ms: e.track.duration_ms,
                        cover_key: None,
                    },
                )?;
                e.track.duration_ms as i64
            }
        };
        let listened = e.listened_ms.min(i64::MAX as u64) as i64;
        if insert.execute(params![id, e.started_at, listened, e.device, e.seq])? == 0 {
            continue;
        }
        n += 1;
        let full = dur > 0 && listened.saturating_mul(10) >= dur.saturating_mul(9);
        let a = added.entry(id).or_insert((0, 0, i64::MIN));
        a.0 += 1;
        a.1 += full as i64;
        a.2 = a.2.max(e.started_at);
    }
    let mut stats = conn.prepare_cached(
        "INSERT INTO play_stats(track_id, play_count, full_play_count, last_played_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(track_id) DO UPDATE SET
            play_count = play_stats.play_count + excluded.play_count,
            full_play_count = play_stats.full_play_count + excluded.full_play_count,
            last_played_at = MAX(COALESCE(play_stats.last_played_at, 0), excluded.last_played_at)",
    )?;
    for (id, (plays, full, last)) in added {
        stats.execute(params![id, plays, full, last])?;
    }
    Ok(n)
}

/// The key as 64 hex digits.
mod hex_key {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    use std::fmt::Write;

    pub fn serialize<S: Serializer>(key: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(64);
        for b in key {
            let _ = write!(out, "{b:02x}");
        }
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        let mut key = [0u8; 32];
        if s.len() != 64 || !s.is_ascii() {
            return Err(D::Error::custom("the key is not 64 hex digits"));
        }
        for (i, b) in key.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(D::Error::custom)?;
        }
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;
    use crate::repo::tests::{db, meta};
    use crate::stats::record;

    fn seed(db: &Db, plays: &[(&str, i64)]) {
        let plays: Vec<(String, i64)> = plays.iter().map(|&(t, at)| (t.to_owned(), at)).collect();
        db.write(move |t| {
            for (id, at) in &plays {
                upsert_track(t, &meta(id, &id.to_uppercase(), ("UCx", "X"), Some(("MPRE1", "One"))))?;
                record(t, &TrackId::from(id.as_str()), *at, 170_000)?;
            }
            Ok(())
        })
        .unwrap();
    }

    /// Copies what `to` lacks from `from`, in batches of `batch`; returns the new plays.
    fn pull(from: &Db, to: &Db, batch: usize) -> usize {
        let mut theirs = to.read(cursors).unwrap();
        let mut n = 0;
        loop {
            let events = from.read(|c| events_after(c, &theirs, batch)).unwrap();
            if events.is_empty() {
                return n;
            }
            advance(&mut theirs, &events);
            n += to.write(move |t| merge_events(t, &events)).unwrap();
        }
    }

    fn total(db: &Db) -> (i64, i64) {
        db.read(|c| {
            Ok(c.query_row(
                "SELECT (SELECT COUNT(*) FROM play_events), (SELECT COALESCE(SUM(play_count), 0) FROM play_stats)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap()
    }

    #[test]
    fn pairing_roundtrip_hides_the_key_from_debug() {
        let (_d, db) = db();
        assert_eq!(db.read(pairing).unwrap(), None);
        let p = Pairing {
            peer_device: "pc".into(),
            peer_name: "Desktop".into(),
            key: [0xab; 32],
            addresses: vec!["192.168.1.5:47821".into()],
            last_address: None,
            paired_at: 1,
            last_exchange: None,
        };
        let p2 = p.clone();
        db.write(move |t| save_pairing(t, &p2)).unwrap();
        db.write(|t| note_exchange(t, Some("192.168.1.5:47821"), 5)).unwrap();
        let got = db.read(pairing).unwrap().unwrap();
        assert_eq!(got.key, p.key);
        assert_eq!((got.last_exchange, got.last_address.as_deref()), (Some(5), Some("192.168.1.5:47821")));
        assert!(!format!("{got:?}").contains("abab"));
        db.write(|t| remove_pairing(t)).unwrap();
        assert_eq!(db.read(pairing).unwrap(), None);
    }

    #[test]
    fn merge_makes_stubs_counts_once_and_resumes() {
        let (_a, pc) = db();
        let (_b, phone) = db();
        seed(&pc, &[("a", 1_000), ("b", 2_000), ("a", 3_000)]);
        seed(&phone, &[("c", 4_000)]);
        // A batch of 2 stops halfway; the rest comes next time.
        let mut theirs = phone.read(cursors).unwrap();
        let first = pc.read(|c| events_after(c, &theirs, 2)).unwrap();
        assert_eq!(first.len(), 2);
        advance(&mut theirs, &first);
        assert_eq!(phone.write(move |t| merge_events(t, &first)).unwrap(), 2);
        assert_eq!(pull(&pc, &phone, 2), 1);
        assert_eq!(pull(&pc, &phone, 2), 0, "nothing new on a repeat");
        assert_eq!(total(&phone), (4, 3), "the phone's own play went through the player only");

        // The stub carries the names; the play counts grew by the new plays only.
        let a = phone.read(|c| crate::repo::track(c, &TrackId::from("a"))).unwrap().unwrap();
        assert_eq!((a.title.as_str(), a.artists[0].name.as_str()), ("A", "X"));
        assert_eq!(a.album.as_ref().map(|al| al.title.as_str()), Some("One"));
        assert_eq!(a.state, mixtape_core::TrackState::Remote);
        let (plays, full, last): (i64, i64, i64) = phone
            .read(|c| {
                Ok(c.query_row(
                    "SELECT play_count, full_play_count, last_played_at FROM play_stats WHERE track_id = 'a'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .unwrap();
        assert_eq!((plays, full, last), (2, 2, 3_000), "170 of 180 s is over 90 %: full");

        // Same log both ways: the PC gets the phone's play and nothing back of its own.
        assert_eq!(pull(&phone, &pc, 1000), 1);
        assert_eq!(phone.read(cursors).unwrap(), pc.read(cursors).unwrap());
        // A merged event keeps its origin, so a third copy is a no-op.
        assert_eq!(pull(&pc, &phone, 1000), 0);
    }
}
