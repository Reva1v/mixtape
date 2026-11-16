//! «Быстрый выбор»: songs worth playing right now, from the play history of all devices.
//!
//! A track scores on three things:
//! - **habit** — how often it is played, recent plays weighing more (half-life [`HABIT_HALF_LIFE`]);
//! - **vibe** — how well it fits what was played lately: it was played in the same listening
//!   sessions as the recent tracks, or shares their artists;
//! - **hour** — it tends to be played at this time of day.
//!
//! What was just heard is left out (it is still in the ear), what was heard today counts less,
//! liked songs a bit more, and no artist takes more than [`PER_ARTIST`] tiles.

use crate::Result;
use mixtape_core::TrackId;
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};

const MIN_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MIN_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

/// How far back the history is read.
const HISTORY: i64 = 180 * DAY_MS;
/// A play shorter than this was a skip, not a listen.
const MIN_LISTEN_MS: i64 = 30_000;
/// A pause longer than this starts a new listening session.
const SESSION_GAP: i64 = 30 * MIN_MS;
/// Plays this recent set the mood: the last session, or the last few hours if it ran long.
const MOOD_WINDOW: i64 = 6 * HOUR_MS;
/// Mood seeds: the most recent distinct tracks.
const MOOD_SEEDS: usize = 15;
const HABIT_HALF_LIFE: f64 = 30.0 * DAY_MS as f64;
/// A track played this recently is left out.
const JUST_HEARD: i64 = 2 * HOUR_MS;
/// A track played this recently (but not just now) counts half.
const HEARD_TODAY: i64 = 18 * HOUR_MS;
/// At most this many tiles per artist.
const PER_ARTIST: usize = 2;

const W_HABIT: f64 = 0.45;
const W_VIBE: f64 = 0.40;
const W_HOUR: f64 = 0.15;
const LIKED_BOOST: f64 = 1.2;

/// One listen.
#[derive(Clone, Debug)]
pub struct Play {
    pub track: String,
    /// Unix ms.
    pub at: i64,
}

/// What [`rank`] needs to know about tracks beyond their plays.
#[derive(Default)]
pub struct Facts {
    /// Artist names by track (lower case).
    pub artists: HashMap<String, Vec<String>>,
    pub liked: HashSet<String>,
    /// Liked tracks never played: they may join when they share the mood's artists.
    pub unplayed_liked: Vec<String>,
}

/// Up to `limit` track ids for «Быстрый выбор», best first.
pub fn smart_pick_ids(
    conn: &Connection,
    now: i64,
    offset_ms: i64,
    limit: usize,
) -> Result<Vec<TrackId>> {
    let plays = plays_since(conn, now - HISTORY)?;
    if plays.is_empty() {
        return Ok(Vec::new());
    }
    let facts = facts(conn)?;
    Ok(rank(&plays, &facts, now, offset_ms, limit)
        .into_iter()
        .map(TrackId::new)
        .collect())
}

fn plays_since(conn: &Connection, since: i64) -> Result<Vec<Play>> {
    let mut stmt = conn.prepare_cached(
        "SELECT track_id, started_at FROM play_events
         WHERE started_at >= ?1 AND listened_ms >= ?2 ORDER BY started_at",
    )?;
    let rows = stmt.query_map(params![since, MIN_LISTEN_MS], |r| {
        Ok(Play {
            track: r.get(0)?,
            at: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn facts(conn: &Connection) -> Result<Facts> {
    let mut f = Facts::default();
    let mut stmt = conn.prepare_cached(
        "SELECT ta.track_id, lower(ta.name) FROM track_artists ta
         WHERE ta.track_id IN (SELECT DISTINCT track_id FROM play_events)
            OR ta.track_id IN (SELECT id FROM tracks WHERE liked = 1)",
    )?;
    for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (t, a) = r?;
        f.artists.entry(t).or_default().push(a);
    }
    let mut stmt = conn.prepare_cached(
        "SELECT id, EXISTS(SELECT 1 FROM play_events p WHERE p.track_id = tracks.id)
         FROM tracks WHERE liked = 1",
    )?;
    for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))? {
        let (t, played) = r?;
        if !played {
            f.unplayed_liked.push(t.clone());
        }
        f.liked.insert(t);
    }
    Ok(f)
}

/// The ranking itself, without the database. `plays` are in time order.
pub fn rank(plays: &[Play], facts: &Facts, now: i64, offset_ms: i64, limit: usize) -> Vec<String> {
    let Some(last) = plays.last() else {
        return Vec::new();
    };
    let no_artists = Vec::new();
    let artists_of = |t: &str| facts.artists.get(t).unwrap_or(&no_artists);

    // Listening sessions: runs of plays without a long pause.
    let mut sessions: Vec<&[Play]> = Vec::new();
    let mut start = 0;
    for i in 1..=plays.len() {
        if i == plays.len() || plays[i].at - plays[i - 1].at > SESSION_GAP {
            sessions.push(&plays[start..i]);
            start = i;
        }
    }

    // The mood: the latest distinct tracks of the current stretch of listening.
    let mood_start = sessions
        .last()
        .map_or(last.at, |s| s[0].at)
        .max(last.at - MOOD_WINDOW);
    let mut seeds: Vec<&str> = Vec::new();
    for p in plays.iter().rev().take_while(|p| p.at >= mood_start) {
        if !seeds.contains(&p.track.as_str()) {
            seeds.push(&p.track);
            if seeds.len() == MOOD_SEEDS {
                break;
            }
        }
    }
    let seed_set: HashSet<&str> = seeds.iter().copied().collect();
    let mut mood_artists: HashMap<&str, f64> = HashMap::new();
    for s in &seeds {
        for a in artists_of(s) {
            *mood_artists.entry(a).or_default() += 1.0;
        }
    }
    // The mood only counts while it is fresh: after a day away it fades out.
    let mood_weight = (-((now - last.at).max(0) as f64) / DAY_MS as f64).exp();

    let mut habit: HashMap<&str, f64> = HashMap::new();
    let mut hour_hits: HashMap<&str, f64> = HashMap::new();
    let mut last_played: HashMap<&str, i64> = HashMap::new();
    let hour_now = local_hour(now, offset_ms);
    for p in plays {
        let age = (now - p.at).max(0) as f64;
        *habit.entry(&p.track).or_default() += 0.5f64.powf(age / HABIT_HALF_LIFE);
        let d = hour_distance(local_hour(p.at, offset_ms), hour_now);
        if d <= 2 {
            *hour_hits.entry(&p.track).or_default() += 1.0 - d as f64 / 3.0;
        }
        last_played.insert(&p.track, p.at);
    }

    // Co-listening: other sessions that held a mood seed vouch for everything else in them.
    let mut co: HashMap<&str, f64> = HashMap::new();
    for s in &sessions {
        if s[0].at >= mood_start {
            continue; // The current session is the mood itself.
        }
        let tracks: HashSet<&str> = s.iter().map(|p| p.track.as_str()).collect();
        let hits = tracks.iter().filter(|t| seed_set.contains(*t)).count();
        if hits == 0 {
            continue;
        }
        let age = (now - s[0].at).max(0) as f64;
        let w = hits as f64 / tracks.len() as f64 * 0.5f64.powf(age / HABIT_HALF_LIFE);
        for t in tracks {
            *co.entry(t).or_default() += w;
        }
    }

    let mut candidates: Vec<&str> = habit.keys().copied().collect();
    candidates.extend(facts.unplayed_liked.iter().map(String::as_str));
    let plays_of: HashMap<&str, f64> = plays.iter().fold(HashMap::new(), |mut m, p| {
        *m.entry(p.track.as_str()).or_default() += 1.0;
        m
    });

    let max = |m: &HashMap<&str, f64>| m.values().copied().fold(0.0, f64::max).max(f64::EPSILON);
    let (habit_max, co_max) = (max(&habit), max(&co));
    let artist_max = mood_artists.values().copied().fold(0.0, f64::max).max(1.0);

    let mut scored: Vec<(&str, f64)> = Vec::new();
    for t in candidates {
        let since = last_played.get(t).map(|at| now - at);
        if since.is_some_and(|s| s < JUST_HEARD) {
            continue;
        }
        let artist_fit = artists_of(t)
            .iter()
            .map(|a| mood_artists.get(a.as_str()).copied().unwrap_or(0.0))
            .fold(0.0, f64::max)
            / artist_max;
        let vibe =
            mood_weight * (0.6 * co.get(t).copied().unwrap_or(0.0) / co_max + 0.4 * artist_fit);
        let hour = hour_hits
            .get(t)
            .map_or(0.0, |h| h / plays_of.get(t).copied().unwrap_or(1.0));
        let mut score = W_HABIT * habit.get(t).copied().unwrap_or(0.0) / habit_max
            + W_VIBE * vibe
            + W_HOUR * hour;
        if facts.liked.contains(t) {
            score *= LIKED_BOOST;
        }
        if since.is_some_and(|s| s < HEARD_TODAY) {
            score *= 0.5;
        }
        if score > 0.0 {
            scored.push((t, score));
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    let mut per_artist: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    for (t, _) in scored {
        let names = artists_of(t);
        if names
            .iter()
            .any(|a| per_artist.get(a.as_str()).copied().unwrap_or(0) >= PER_ARTIST)
        {
            continue;
        }
        for a in names {
            *per_artist.entry(a).or_default() += 1;
        }
        out.push(t.to_owned());
        if out.len() == limit {
            break;
        }
    }
    out
}

fn local_hour(at: i64, offset_ms: i64) -> i64 {
    (at + offset_ms).rem_euclid(DAY_MS) / HOUR_MS
}

/// Hours between two hours of the day, around midnight too.
fn hour_distance(a: i64, b: i64) -> i64 {
    let d = (a - b).abs();
    d.min(24 - d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::{db, meta};
    use crate::repo::upsert_track;
    use crate::stats::record;

    /// 2026-10-12 20:00 UTC.
    const NOW: i64 = 1_791_763_200_000 + 20 * HOUR_MS;

    fn play(t: &str, at: i64) -> Play {
        Play {
            track: t.into(),
            at,
        }
    }

    fn facts(artists: &[(&str, &str)]) -> Facts {
        Facts {
            artists: artists
                .iter()
                .map(|(t, a)| (t.to_string(), vec![a.to_string()]))
                .collect(),
            ..Facts::default()
        }
    }

    #[test]
    fn empty_history_gives_nothing() {
        assert!(rank(&[], &Facts::default(), NOW, 0, 9).is_empty());
    }

    #[test]
    fn often_played_beats_once_played() {
        let mut plays = Vec::new();
        for d in 1..=10 {
            plays.push(play("fav", NOW - d * DAY_MS));
        }
        plays.push(play("once", NOW - 5 * DAY_MS));
        plays.sort_by_key(|p| p.at);
        let f = facts(&[("fav", "a"), ("once", "b")]);
        assert_eq!(rank(&plays, &f, NOW, 0, 9), ["fav", "once"]);
    }

    #[test]
    fn just_heard_is_left_out_and_today_counts_less() {
        let plays = vec![
            play("today", NOW - 10 * HOUR_MS),
            play("today", NOW - 9 * HOUR_MS),
            play("old", NOW - 3 * DAY_MS),
            play("now", NOW - 20 * MIN_MS),
        ];
        let f = facts(&[("now", "a"), ("today", "b"), ("old", "c")]);
        let out = rank(&plays, &f, NOW, 0, 9);
        assert!(!out.contains(&"now".to_string()), "{out:?}");
        assert!(out.contains(&"today".to_string()));
    }

    #[test]
    fn the_mood_pulls_in_what_was_heard_with_it() {
        // Long ago: a session of rock (r1 r2 r3) and one of pop (p1 p2).
        let mut plays = vec![
            play("r1", NOW - 20 * DAY_MS),
            play("r2", NOW - 20 * DAY_MS + 4 * MIN_MS),
            play("r3", NOW - 20 * DAY_MS + 8 * MIN_MS),
            play("p1", NOW - 19 * DAY_MS),
            play("p2", NOW - 19 * DAY_MS + 4 * MIN_MS),
            play("p2", NOW - 18 * DAY_MS),
        ];
        // Tonight: r1, played 3 hours ago, sets a rock mood.
        plays.push(play("r1", NOW - 3 * HOUR_MS));
        let f = facts(&[
            ("r1", "x"),
            ("r2", "y"),
            ("r3", "z"),
            ("p1", "p"),
            ("p2", "q"),
        ]);
        let out = rank(&plays, &f, NOW, 0, 3);
        assert!(
            out.contains(&"r2".to_string()) && out.contains(&"r3".to_string()),
            "{out:?}"
        );
    }

    #[test]
    fn the_moods_artist_brings_an_unplayed_liked_song() {
        let plays = vec![
            play("other", NOW - 2 * DAY_MS),
            play("a1", NOW - 3 * HOUR_MS),
        ];
        let mut f = facts(&[
            ("a1", "artist"),
            ("a2", "artist"),
            ("other", "o"),
            ("stranger", "s"),
        ]);
        f.liked = ["a2", "stranger"].into_iter().map(String::from).collect();
        f.unplayed_liked = vec!["a2".into(), "stranger".into()];
        let out = rank(&plays, &f, NOW, 0, 9);
        assert!(out.contains(&"a2".to_string()), "{out:?}");
        assert!(
            !out.contains(&"stranger".to_string()),
            "an unrelated unplayed song stays out: {out:?}"
        );
    }

    #[test]
    fn no_artist_takes_more_than_two_tiles() {
        let plays: Vec<Play> = (0..5)
            .map(|i| play(&format!("t{i}"), NOW - (5 - i) * DAY_MS))
            .collect();
        let f = facts(&[
            ("t0", "a"),
            ("t1", "a"),
            ("t2", "a"),
            ("t3", "a"),
            ("t4", "a"),
        ]);
        assert_eq!(rank(&plays, &f, NOW, 0, 9).len(), PER_ARTIST);
    }

    #[test]
    fn the_hour_of_day_tips_the_balance() {
        // One play each, days ago (the mood has faded); "evening" was played at 20:00 UTC,
        // "morning" at 08:00.
        let plays = vec![
            play("morning", NOW - 5 * DAY_MS - 12 * HOUR_MS),
            play("evening", NOW - 5 * DAY_MS),
        ];
        let f = facts(&[("morning", "a"), ("evening", "b")]);
        assert_eq!(rank(&plays, &f, NOW, 0, 9)[0], "evening");
        // Twelve hours later it is 08:00: the morning song comes first.
        assert_eq!(rank(&plays, &f, NOW + 12 * HOUR_MS, 0, 9)[0], "morning");
        // The same at UTC+3, where both plays and now shift by the offset alike.
        assert_eq!(rank(&plays, &f, NOW + 12 * HOUR_MS, 3 * HOUR_MS, 9)[0], "morning");
    }

    #[test]
    fn limit_is_respected() {
        let plays: Vec<Play> = (0..20)
            .map(|i| play(&format!("t{i}"), NOW - (30 - i) * DAY_MS))
            .collect();
        let f = Facts {
            artists: (0..20)
                .map(|i| (format!("t{i}"), vec![format!("a{i}")]))
                .collect(),
            ..Facts::default()
        };
        assert_eq!(rank(&plays, &f, NOW, 0, 8).len(), 8);
    }

    #[test]
    fn reads_the_database_skips_and_all() {
        let (_d, db) = db();
        db.write(|c| {
            upsert_track(c, &meta("a", "A", ("UCx", "X"), None))?;
            upsert_track(c, &meta("b", "B", ("UCy", "Y"), None))?;
            upsert_track(c, &meta("s", "S", ("UCz", "Z"), None))?;
            for d in 1..=3 {
                record(c, &TrackId::from("a"), NOW - d * DAY_MS, 180_000)?;
            }
            record(c, &TrackId::from("b"), NOW - 4 * DAY_MS, 180_000)?;
            // A 5-second skip is not a listen.
            record(c, &TrackId::from("s"), NOW - DAY_MS, 5_000)?;
            let ids = smart_pick_ids(c, NOW, 0, 9)?;
            assert_eq!(ids, [TrackId::from("a"), TrackId::from("b")]);
            Ok(())
        })
        .unwrap();
    }
}
