//! Recent searches, the same on the phone and the PC.
//!
//! The list is a set of records keyed by what they are ([`RecentItem::key`]): a query by its
//! trimmed, lower-cased text, anything else by kind and id. A record is a use (`item`) or a
//! removal (a tombstone: `item = None`), stamped with when it happened. Two devices merge by
//! keeping, per key, the newer record, so a removal on one device doesn't come back from the
//! other and a new use beats an older removal. A clear raises a watermark ([`Snapshot::cleared_at`]):
//! everything not newer than it is gone, including entries the other device made but this one
//! never saw. Only the newest [`MAX_LIVE`] uses are kept; tombstones live [`TOMBSTONE_TTL_MS`].
//!
//! The merge is pure ([`merge`]) and the storage is a table plus one setting (migration 010):
//! a database operation loads the [`Snapshot`], applies the pure change and stores it back.
//! The wire protocol sends the whole snapshot (`mixtape_sync::peer`).

use crate::Result;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;

/// Live entries kept.
pub const MAX_LIVE: usize = 20;
/// Tombstones older than this are dropped (a device offline for longer may bring an old
/// entry back).
pub const TOMBSTONE_TTL_MS: i64 = 60 * 24 * 60 * 60 * 1000;
/// Tombstones kept at most, newest first: bounds the snapshot after a burst of removals.
pub const MAX_TOMBSTONES: usize = 200;
/// A query is cut to this many characters.
pub const MAX_QUERY_CHARS: usize = 200;

/// The `settings` key of the clear watermark ([`Snapshot::cleared_at`], a JSON number).
const CLEARED_KEY: &str = "recent_cleared_at";
/// The key the Android app stored its plain list of queries under, before this module.
pub const LEGACY_KEY: &str = "search.recent";

/// What something shown under the search box refers to; the JSON is what the interface uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RecentItem {
    Query { text: String },
    Track(Entity),
    Album(Entity),
    Artist(Entity),
    Playlist(Entity),
}

/// A catalogue or library thing the user opened from search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    pub id: String,
    pub title: String,
    /// Artists; the kind label is added when shown, in the current language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_key: Option<String>,
}

impl RecentItem {
    /// A query item.
    pub fn query(text: impl Into<String>) -> Self {
        Self::Query { text: text.into() }
    }

    /// The identity the lists dedupe and merge by: `q:<trimmed lower-case text with single
    /// spaces>` or `<kind>:<id>`.
    pub fn key(&self) -> String {
        match self {
            Self::Query { text } => format!("q:{}", normalize_query(text)),
            Self::Track(e) => format!("track:{}", e.id),
            Self::Album(e) => format!("album:{}", e.id),
            Self::Artist(e) => format!("artist:{}", e.id),
            Self::Playlist(e) => format!("playlist:{}", e.id),
        }
    }

    /// Whether it can be shown: a query needs text, the rest an id.
    fn is_valid(&self) -> bool {
        match self {
            Self::Query { text } => !normalize_query(text).is_empty(),
            Self::Track(e) | Self::Album(e) | Self::Artist(e) | Self::Playlist(e) => {
                !e.id.is_empty()
            }
        }
    }

    /// The same item with a query's text trimmed and cut.
    fn tidy(mut self) -> Self {
        if let Self::Query { text } = &mut self {
            let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
            *text = t.chars().take(MAX_QUERY_CHARS).collect();
        }
        self
    }
}

/// `text` trimmed, whitespace runs collapsed to one space, lower-cased.
pub fn normalize_query(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// One entry of the list: a use of `item`, or (without it) a removal of `key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub key: String,
    /// When the item was last used or removed, Unix ms.
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<RecentItem>,
}

impl Record {
    fn is_live(&self) -> bool {
        self.item.is_some()
    }

    /// Which of two records of one key stands: the newer; at the same time a removal beats a
    /// use, and two uses are told apart by their JSON, so that the merge doesn't depend on the order.
    fn cmp_rank(&self, other: &Self) -> Ordering {
        self.at
            .cmp(&other.at)
            .then_with(|| other.is_live().cmp(&self.is_live()))
            .then_with(|| json(&self.item).cmp(&json(&other.item)))
    }
}

fn json(item: &Option<RecentItem>) -> String {
    serde_json::to_string(item).unwrap_or_default()
}

/// The whole state of the list: what goes over the wire and what the pure operations work on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Everything not newer than this was cleared (Unix ms; 0 = never).
    #[serde(default)]
    pub cleared_at: i64,
    #[serde(default)]
    pub records: Vec<Record>,
}

impl Snapshot {
    /// The uses, newest first.
    pub fn live(&self) -> Vec<RecentItem> {
        let mut live: Vec<&Record> = self
            .records
            .iter()
            .filter(|r| r.is_live() && r.at > self.cleared_at)
            .collect();
        live.sort_by(|a, b| b.cmp_rank(a));
        live.into_iter()
            .filter_map(|r| r.item.clone())
            .take(MAX_LIVE)
            .collect()
    }

    /// Records `item` as used at `now`. An item that can't be shown (an empty query) changes
    /// nothing. Returns whether the snapshot changed.
    pub fn add(&mut self, item: RecentItem, now: i64) -> bool {
        let item = item.tidy();
        if !item.is_valid() {
            return false;
        }
        let key = item.key();
        // Later than a removal or a clear made here, even if the clock stepped back.
        let floor = self.cleared_at.max(
            self.records
                .iter()
                .find(|r| r.key == key)
                .map_or(0, |r| r.at),
        );
        let at = now.max(floor + 1);
        self.records.retain(|r| r.key != key);
        self.records.push(Record {
            key,
            at,
            item: Some(item),
        });
        self.settle(now);
        true
    }

    /// Records the removal of `item` at `now`. Returns whether it was listed.
    pub fn remove(&mut self, item: &RecentItem, now: i64) -> bool {
        let key = item.key();
        let listed = self
            .records
            .iter()
            .any(|r| r.key == key && r.is_live() && r.at > self.cleared_at);
        if !listed {
            return false;
        }
        let at = self
            .records
            .iter()
            .find(|r| r.key == key)
            .map_or(now, |r| now.max(r.at + 1));
        self.records.retain(|r| r.key != key);
        self.records.push(Record {
            key,
            at,
            item: None,
        });
        self.settle(now);
        true
    }

    /// Removes everything, here and on the devices that merge with this snapshot later.
    /// Returns whether anything was listed.
    pub fn clear(&mut self, now: i64) -> bool {
        let listed = self
            .records
            .iter()
            .any(|r| r.is_live() && r.at > self.cleared_at);
        if !listed {
            return false;
        }
        let newest = self.records.iter().map(|r| r.at).max().unwrap_or(0);
        self.cleared_at = now.max(newest).max(self.cleared_at);
        self.settle(now);
        true
    }

    /// Adds `items` (newest first) that aren't listed yet, as if they had been used just
    /// before `now`, one after the other: the old formats had no times. Returns whether
    /// anything was added.
    pub fn import(&mut self, items: Vec<RecentItem>, now: i64) -> bool {
        let mut changed = false;
        for (i, item) in items.into_iter().enumerate() {
            let item = item.tidy();
            let at = now - i as i64;
            if !item.is_valid() || at <= self.cleared_at {
                continue;
            }
            let key = item.key();
            if self.records.iter().any(|r| r.key == key) {
                continue;
            }
            self.records.push(Record {
                key,
                at,
                item: Some(item),
            });
            changed = true;
        }
        if changed {
            self.settle(now);
        }
        changed
    }

    /// Drops what the watermark covers, old and surplus records, and puts the rest in a
    /// fixed order (newest first), so equal states compare equal.
    fn settle(&mut self, now: i64) {
        let cleared = self.cleared_at;
        let oldest_tombstone = now.saturating_sub(TOMBSTONE_TTL_MS);
        self.records
            .retain(|r| r.at > cleared && (r.is_live() || r.at >= oldest_tombstone));
        self.records
            .sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.key.cmp(&b.key)));
        let (mut live, mut dead) = (0, 0);
        self.records.retain(|r| {
            let n = if r.is_live() { &mut live } else { &mut dead };
            *n += 1;
            *n <= if r.is_live() {
                MAX_LIVE
            } else {
                MAX_TOMBSTONES
            }
        });
    }
}

/// Merges two snapshots as the exchange does: per key the newer record, the later clear,
/// then the limits. `merge(a, b) == merge(b, a)` and merging again changes nothing. Records
/// are taken by the key their item says, so a peer that normalises differently can't split
/// an entry in two.
pub fn merge(a: &Snapshot, b: &Snapshot, now: i64) -> Snapshot {
    let mut by_key: HashMap<String, Record> = HashMap::new();
    for r in a.records.iter().chain(&b.records) {
        let key = match &r.item {
            Some(item) if !item.is_valid() => continue,
            Some(item) => item.key(),
            None => r.key.clone(),
        };
        let r = Record {
            key: key.clone(),
            ..r.clone()
        };
        match by_key.get(&key) {
            Some(held) if held.cmp_rank(&r) != Ordering::Less => {}
            _ => {
                by_key.insert(key, r);
            }
        }
    }
    let mut merged = Snapshot {
        cleared_at: a.cleared_at.max(b.cleared_at),
        records: by_key.into_values().collect(),
    };
    merged.settle(now);
    merged
}

// ---- Storage ----

/// The stored snapshot.
pub fn snapshot(conn: &Connection) -> Result<Snapshot> {
    let cleared_at: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [CLEARED_KEY],
            |r| r.get(0),
        )
        .optional()?;
    let cleared_at = cleared_at
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(0);
    let mut stmt = conn.prepare("SELECT key, at, item FROM recent_search")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut records = Vec::new();
    for row in rows {
        let (key, at, item) = row?;
        // A row that doesn't parse (a newer version's kind) is skipped, not fatal.
        let item = match item {
            Some(raw) => match serde_json::from_str::<RecentItem>(&raw) {
                Ok(item) => Some(item),
                Err(_) => continue,
            },
            None => None,
        };
        records.push(Record { key, at, item });
    }
    Ok(Snapshot {
        cleared_at,
        records,
    })
}

/// The list, newest first.
pub fn list(conn: &Connection) -> Result<Vec<RecentItem>> {
    Ok(snapshot(conn)?.live())
}

/// Replaces the stored snapshot.
fn store(t: &Transaction<'_>, s: &Snapshot) -> Result<()> {
    t.execute("DELETE FROM recent_search", [])?;
    let mut insert = t.prepare("INSERT INTO recent_search(key, at, item) VALUES (?1, ?2, ?3)")?;
    for r in &s.records {
        let item = r.item.as_ref().map(serde_json::to_string).transpose()?;
        insert.execute(params![r.key, r.at, item])?;
    }
    t.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![CLEARED_KEY, s.cleared_at.to_string()],
    )?;
    Ok(())
}

/// Loads, changes and stores; the list afterwards, newest first.
fn change(t: &Transaction<'_>, f: impl FnOnce(&mut Snapshot) -> bool) -> Result<Vec<RecentItem>> {
    let mut s = snapshot(t)?;
    if f(&mut s) {
        store(t, &s)?;
    }
    Ok(s.live())
}

/// Lists `item` as used at `now` (moving it to the top); returns the list.
pub fn add(t: &Transaction<'_>, item: RecentItem, now: i64) -> Result<Vec<RecentItem>> {
    change(t, |s| s.add(item, now))
}

/// Removes `item` (it stays removed on the other device after the next exchange).
pub fn remove(t: &Transaction<'_>, item: &RecentItem, now: i64) -> Result<Vec<RecentItem>> {
    change(t, |s| s.remove(item, now))
}

/// Removes everything.
pub fn clear(t: &Transaction<'_>, now: i64) -> Result<Vec<RecentItem>> {
    change(t, |s| s.clear(now))
}

/// Adds items from an older format (newest first) that aren't listed yet; returns the list.
pub fn import(t: &Transaction<'_>, items: Vec<RecentItem>, now: i64) -> Result<Vec<RecentItem>> {
    change(t, |s| s.import(items, now))
}

/// Merges what the other device sent. Returns whether the stored list changed.
pub fn merge_remote(t: &Transaction<'_>, remote: &Snapshot, now: i64) -> Result<bool> {
    let local = snapshot(t)?;
    let merged = merge(&local, remote, now);
    // The stored rows come in any order and may hold what only the limits drop (an old
    // tombstone): compare with them put in order, so that is not news.
    let mut settled = local.clone();
    settled.settle(now);
    if merged != settled {
        store(t, &merged)?;
    }
    Ok(merged.live() != local.live())
}

/// Once, when a database is opened: the Android app's old list (`settings` key
/// [`LEGACY_KEY`], a JSON array of query texts, newest first) becomes query entries, and the
/// old key is deleted. Nothing happens when the key isn't there.
pub fn import_legacy_queries(conn: &mut Connection) -> Result<()> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [LEGACY_KEY],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw else { return Ok(()) };
    let texts: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
    let t = conn.transaction()?;
    import(
        &t,
        texts.into_iter().map(RecentItem::query).collect(),
        crate::repo::now_ms(),
    )?;
    t.execute("DELETE FROM settings WHERE key = ?1", [LEGACY_KEY])?;
    t.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;

    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn q(text: &str) -> RecentItem {
        RecentItem::query(text)
    }

    fn track(id: &str) -> RecentItem {
        RecentItem::Track(Entity {
            id: id.into(),
            title: format!("T {id}"),
            subtitle: Some("Artist".into()),
            cover_key: None,
        })
    }

    fn texts(items: &[RecentItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                RecentItem::Query { text } => text.clone(),
                RecentItem::Track(e)
                | RecentItem::Album(e)
                | RecentItem::Artist(e)
                | RecentItem::Playlist(e) => e.id.clone(),
            })
            .collect()
    }

    fn snap(build: impl FnOnce(&mut Snapshot)) -> Snapshot {
        let mut s = Snapshot::default();
        build(&mut s);
        s
    }

    // ---- Keys and adding ----

    #[test]
    fn a_query_is_deduped_by_its_normalised_text() {
        let mut s = Snapshot::default();
        s.add(q("Loser"), 1000);
        s.add(q("  other "), 2000);
        s.add(q("loser"), 3000);
        s.add(q("LOSER  "), 4000);
        // The newest spelling stands and the entry moved to the top.
        assert_eq!(texts(&s.live()), ["LOSER", "other"]);
        assert_eq!(s.records.len(), 2);
        assert_eq!(q("a   B\tc").key(), "q:a b c");
        assert_eq!(q("Привіт").key(), q("ПРИВІТ ").key());
    }

    #[test]
    fn items_of_other_kinds_are_keyed_by_kind_and_id() {
        let mut s = Snapshot::default();
        let album = RecentItem::Album(Entity {
            id: "1".into(),
            title: "A".into(),
            subtitle: None,
            cover_key: None,
        });
        s.add(track("1"), 1);
        s.add(album, 2);
        s.add(track("1"), 3);
        assert_eq!(s.live().len(), 2);
        assert_eq!(s.live()[0], track("1"));
        // Same id under another kind is another entry; the same key from a newer use updates the title.
        let mut renamed = track("1");
        if let RecentItem::Track(e) = &mut renamed {
            e.title = "Renamed".into();
        }
        s.add(renamed.clone(), 4);
        assert_eq!(s.live()[0], renamed);
    }

    #[test]
    fn an_empty_query_or_id_is_ignored() {
        let mut s = Snapshot::default();
        assert!(!s.add(q("   "), 1));
        assert!(!s.add(
            RecentItem::Track(Entity {
                id: String::new(),
                title: "x".into(),
                subtitle: None,
                cover_key: None
            }),
            1
        ));
        assert_eq!(s, Snapshot::default());
    }

    #[test]
    fn only_the_newest_twenty_are_kept() {
        let mut s = Snapshot::default();
        for i in 0..25 {
            s.add(q(&format!("query {i}")), 1000 + i);
        }
        let live = texts(&s.live());
        assert_eq!(live.len(), MAX_LIVE);
        assert_eq!(live[0], "query 24");
        assert_eq!(live[19], "query 5");
        assert_eq!(s.records.len(), MAX_LIVE);
    }

    #[test]
    fn a_long_query_is_cut() {
        let mut s = Snapshot::default();
        s.add(q(&"x".repeat(500)), 1);
        assert_eq!(texts(&s.live())[0].chars().count(), MAX_QUERY_CHARS);
    }

    #[test]
    fn adding_after_the_clock_stepped_back_still_wins() {
        let mut s = Snapshot::default();
        s.add(q("a"), 10_000);
        s.remove(&q("a"), 10_001);
        // `now` is earlier than the tombstone.
        s.add(q("a"), 5_000);
        assert_eq!(texts(&s.live()), ["a"]);
    }

    // ---- Remove and clear ----

    #[test]
    fn removing_leaves_a_tombstone_and_clear_a_watermark() {
        let mut s = Snapshot::default();
        s.add(q("a"), 1000);
        s.add(q("b"), 2000);
        assert!(s.remove(&q("A"), 3000));
        assert!(!s.remove(&q("a"), 3500), "already gone");
        assert!(!s.remove(&q("nope"), 3500));
        assert_eq!(texts(&s.live()), ["b"]);
        assert!(s.records.iter().any(|r| r.key == "q:a" && r.item.is_none()));

        assert!(s.clear(4000));
        assert!(s.live().is_empty());
        assert_eq!(s.cleared_at, 4000);
        assert!(s.records.is_empty(), "the watermark covers the records");
        assert!(!s.clear(5000), "nothing to clear");
        assert_eq!(s.cleared_at, 4000);
        // Something new after the clear is listed again, even in the same millisecond.
        s.add(q("c"), 4000);
        assert_eq!(texts(&s.live()), ["c"]);
    }

    #[test]
    fn old_tombstones_are_pruned_but_new_ones_stay() {
        let mut s = Snapshot::default();
        s.add(q("old"), 1000);
        s.remove(&q("old"), 2000);
        s.add(q("recent"), 3000);
        s.remove(&q("recent"), 3000 + DAY);
        s.add(q("x"), 2000 + TOMBSTONE_TTL_MS + 10);
        let keys: Vec<_> = s.records.iter().map(|r| r.key.as_str()).collect();
        assert!(!keys.contains(&"q:old"), "{keys:?}");
        assert!(keys.contains(&"q:recent"), "{keys:?}");
    }

    #[test]
    fn tombstones_are_capped() {
        let mut s = Snapshot::default();
        for i in 0..300 {
            s.add(q(&format!("q{i}")), 1000 + i);
            s.remove(&q(&format!("q{i}")), 5000 + i);
        }
        assert_eq!(s.records.len(), MAX_TOMBSTONES);
        // The newest removals are the ones kept.
        assert!(s.records.iter().any(|r| r.key == "q:q299"));
        assert!(!s.records.iter().any(|r| r.key == "q:q0"));
    }

    // ---- Merge ----

    #[test]
    fn the_union_keeps_the_newer_spelling_and_order() {
        let a = snap(|s| {
            s.add(q("Loser"), 1000);
            s.add(q("Beck"), 3000);
        });
        let b = snap(|s| {
            s.add(q("loser"), 2000);
            s.add(q("Oasis"), 4000);
        });
        let m = merge(&a, &b, 10_000);
        assert_eq!(texts(&m.live()), ["Oasis", "Beck", "loser"]);
    }

    #[test]
    fn a_tombstone_beats_an_older_entry_and_loses_to_a_newer_one() {
        let on_pc = snap(|s| {
            s.add(q("x"), 1000);
            s.add(q("y"), 1000);
            s.remove(&q("x"), 5000);
        });
        let on_phone = snap(|s| {
            s.add(q("x"), 2000); // older than the removal
            s.add(q("y"), 1000);
            s.remove(&q("y"), 1500);
            s.add(q("y"), 6000); // newer than the removal
        });
        let m = merge(&on_pc, &on_phone, 10_000);
        assert_eq!(texts(&m.live()), ["y"]);
        assert!(m.records.iter().any(|r| r.key == "q:x" && r.item.is_none()));
        // And when the use is the newer one.
        let on_phone = snap(|s| {
            s.add(q("x"), 9000);
        });
        assert_eq!(texts(&merge(&on_pc, &on_phone, 10_000).live()), ["x", "y"]);
    }

    #[test]
    fn a_clear_removes_what_the_other_device_never_saw() {
        let pc = snap(|s| {
            s.add(q("a"), 1000);
            s.add(q("b"), 2000);
            s.clear(3000);
        });
        let phone = snap(|s| {
            s.add(q("a"), 1000);
            s.add(q("c"), 2500); // made on the phone before the clear, not synced yet
            s.add(q("d"), 4000); // made after it
        });
        let m = merge(&pc, &phone, 10_000);
        assert_eq!(m.cleared_at, 3000);
        assert_eq!(texts(&m.live()), ["d"]);
    }

    #[test]
    fn equal_times_resolve_the_same_way_in_both_orders() {
        let live = snap(|s| {
            s.records.push(Record {
                key: "q:x".into(),
                at: 100,
                item: Some(q("x")),
            })
        });
        let live_upper = snap(|s| {
            s.records.push(Record {
                key: "q:x".into(),
                at: 100,
                item: Some(q("X")),
            })
        });
        let dead = snap(|s| {
            s.records.push(Record {
                key: "q:x".into(),
                at: 100,
                item: None,
            })
        });
        for (a, b) in [(&live, &live_upper), (&live, &dead), (&live_upper, &dead)] {
            assert_eq!(merge(a, b, 1000), merge(b, a, 1000));
        }
        // The removal wins a tie with a use.
        assert!(merge(&live, &dead, 1000).live().is_empty());
    }

    #[test]
    fn merge_is_commutative_and_idempotent() {
        let a = snap(|s| {
            for i in 0..15 {
                s.add(q(&format!("a{i}")), 1000 + i * 10);
            }
            s.remove(&q("a3"), 2000);
            s.add(track("t1"), 1500);
        });
        let b = snap(|s| {
            for i in 0..15 {
                s.add(q(&format!("b{i}")), 1005 + i * 10);
            }
            s.add(q("A3"), 1100);
            s.remove(&q("b0"), 1006);
            s.add(track("t1"), 1600);
        });
        let ab = merge(&a, &b, 10_000);
        assert_eq!(ab, merge(&b, &a, 10_000));
        assert_eq!(ab.live().len(), MAX_LIVE);
        // Merging again, with either side or itself, changes nothing.
        assert_eq!(merge(&ab, &a, 10_000), ab);
        assert_eq!(merge(&ab, &b, 10_000), ab);
        assert_eq!(merge(&ab, &ab, 10_000), ab);
        assert_eq!(
            merge(&a, &a, 10_000),
            merge(&a, &Snapshot::default(), 10_000)
        );
        // The cap drops the oldest of the union.
        assert!(!texts(&ab.live()).contains(&"a0".to_owned()));
    }

    #[test]
    fn merge_rekeys_by_the_item_and_drops_unshowable_records() {
        let odd = snap(|s| {
            s.records.push(Record {
                key: "q:Wrong Key".into(),
                at: 5,
                item: Some(q("Loser")),
            });
            s.records.push(Record {
                key: "q:".into(),
                at: 6,
                item: Some(q("  ")),
            });
        });
        let m = merge(&odd, &Snapshot::default(), 100);
        assert_eq!(m.records.len(), 1);
        assert_eq!(m.records[0].key, "q:loser");
    }

    #[test]
    fn snapshot_json_roundtrips_and_reads_what_the_interface_sends() {
        let s = snap(|s| {
            s.add(q("Loser"), 1000);
            s.add(track("t1"), 2000);
            s.remove(&q("loser"), 3000);
        });
        let back: Snapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        // The interface's own shapes, including `coverKey: null` and a missing subtitle.
        let item: RecentItem = serde_json::from_str(
            r#"{"kind":"album","id":"MPRE1","title":"Odelay","coverKey":null}"#,
        )
        .unwrap();
        assert_eq!(item.key(), "album:MPRE1");
        let item: RecentItem = serde_json::from_str(r#"{"kind":"query","text":"Loser"}"#).unwrap();
        assert_eq!(item, q("Loser"));
        assert_eq!(
            serde_json::to_string(&q("a")).unwrap(),
            r#"{"kind":"query","text":"a"}"#
        );
        // A snapshot from a peer that sends nothing else.
        let empty: Snapshot = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Snapshot::default());
    }

    // ---- Storage ----

    fn open() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn the_database_keeps_the_list_between_operations() {
        let (_d, db) = open();
        db.write(|t| add(t, q("Loser"), 1000).map(drop)).unwrap();
        db.write(|t| add(t, track("t1"), 2000).map(drop)).unwrap();
        db.write(|t| add(t, q("loser"), 3000).map(drop)).unwrap();
        assert_eq!(texts(&db.read(list).unwrap()), ["loser", "t1"]);
        let after = db.write(|t| remove(t, &track("t1"), 4000)).unwrap();
        assert_eq!(texts(&after), ["loser"]);
        assert_eq!(texts(&db.read(list).unwrap()), ["loser"]);
        let after = db.write(|t| clear(t, 5000)).unwrap();
        assert!(after.is_empty());
        let s = db.read(snapshot).unwrap();
        assert_eq!((s.cleared_at, s.records.len()), (5000, 0));
        // A clear on an empty list is a no-op, and survives a reopen as such.
        assert!(db.write(|t| clear(t, 6000)).unwrap().is_empty());
        assert_eq!(db.read(snapshot).unwrap().cleared_at, 5000);
    }

    #[test]
    fn two_databases_converge_through_merge_remote() {
        let (_a, pc) = open();
        let (_b, phone) = open();
        let now = 100_000;
        phone.write(|t| add(t, q("Loser"), 1000).map(drop)).unwrap();
        pc.write(|t| add(t, q("Beck"), 2000).map(drop)).unwrap();
        pc.write(|t| add(t, q("gone"), 2100).map(drop)).unwrap();
        pc.write(|t| remove(t, &q("gone"), 2200).map(drop)).unwrap();
        phone.write(|t| add(t, q("gone"), 1500).map(drop)).unwrap();

        let from_phone = phone.read(snapshot).unwrap();
        assert!(
            pc.write(move |t| merge_remote(t, &from_phone, now))
                .unwrap()
        );
        let from_pc = pc.read(snapshot).unwrap();
        assert!(
            phone
                .write(move |t| merge_remote(t, &from_pc, now))
                .unwrap()
        );
        assert_eq!(texts(&pc.read(list).unwrap()), ["Beck", "Loser"]);
        assert_eq!(texts(&phone.read(list).unwrap()), ["Beck", "Loser"]);
        assert_eq!(pc.read(snapshot).unwrap(), phone.read(snapshot).unwrap());
        // A second round has nothing new on either side.
        let from_phone = phone.read(snapshot).unwrap();
        assert!(
            !pc.write(move |t| merge_remote(t, &from_phone, now))
                .unwrap()
        );
    }

    #[test]
    fn an_old_tombstone_arriving_is_not_news() {
        let (_d, db) = open();
        let remote = Snapshot {
            cleared_at: 0,
            records: vec![Record {
                key: "q:x".into(),
                at: 1000,
                item: None,
            }],
        };
        let changed = db
            .write(move |t| merge_remote(t, &remote, 1000 + TOMBSTONE_TTL_MS + 1))
            .unwrap();
        assert!(!changed);
        assert!(db.read(snapshot).unwrap().records.is_empty());
    }

    // ---- Old formats ----

    #[test]
    fn the_android_list_of_strings_becomes_query_entries_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let db = Db::open(&path).unwrap();
            crate::settings::set(&db, LEGACY_KEY, &["Loser", "  beck", "loser", "", "Oasis"])
                .unwrap();
        }
        // The next open migrates it: duplicates by normalised text collapse, the order stays.
        let db = Db::open(&path).unwrap();
        assert_eq!(texts(&db.read(list).unwrap()), ["Loser", "beck", "Oasis"]);
        assert_eq!(
            crate::settings::get::<Vec<String>>(&db, LEGACY_KEY).unwrap(),
            None
        );
        // Opening again doesn't re-import what the user removed since.
        db.write(|t| remove(t, &q("beck"), crate::repo::now_ms() + 10).map(drop))
            .unwrap();
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(texts(&db.read(list).unwrap()), ["Loser", "Oasis"]);
    }

    #[test]
    fn import_adds_only_what_is_missing_and_keeps_order() {
        let mut s = Snapshot::default();
        s.add(q("Loser"), 5000);
        s.remove(&q("Loser"), 6000);
        s.add(q("kept"), 6500);
        // `Loser` was removed on purpose: an older format doesn't bring it back.
        assert!(s.import(vec![q("loser"), q("first"), q("second"), q("KEPT")], 7000));
        assert_eq!(texts(&s.live()), ["first", "second", "kept"]);
        assert!(!s.import(vec![q("first")], 8000));
        // A list that is garbage or empty is harmless.
        assert!(!Snapshot::default().import(vec![q("  ")], 1));
    }

    #[test]
    fn a_broken_legacy_value_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let db = Db::open(&path).unwrap();
            db.write(|t| {
                t.execute(
                    "INSERT INTO settings(key, value) VALUES (?1, 'not json')",
                    [LEGACY_KEY],
                )?;
                Ok(())
            })
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert!(db.read(list).unwrap().is_empty());
        assert_eq!(
            crate::settings::get::<serde_json::Value>(&db, LEGACY_KEY).unwrap(),
            None
        );
    }

    #[test]
    fn newest_first_ordering_is_stable_for_equal_times() {
        let mut s = Snapshot::default();
        s.records.push(Record {
            key: "q:b".into(),
            at: 10,
            item: Some(q("b")),
        });
        s.records.push(Record {
            key: "q:a".into(),
            at: 10,
            item: Some(q("a")),
        });
        let first = texts(&s.live());
        s.records.reverse();
        assert_eq!(first, texts(&s.live()));
    }
}
