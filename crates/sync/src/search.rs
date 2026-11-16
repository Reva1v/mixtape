//! Catalog search without the wait for pictures, shared by the desktop and the Android app.
//!
//! A search answers as soon as YouTube's reply is parsed and its rows are in the library
//! database. The thumbnails are fetched afterwards by one background task with one limit and
//! announced one by one (`CoverSink::on_ready`), so the screen can draw them when they land.
//! Around that sit a small result cache (same query and filter within [`CACHE_TTL`]) and a
//! gate that cancels a search, with its pictures, when a newer one starts.

use crate::{catalog, sources};
use mixtape_artwork::{CoverStore, Detail};
use mixtape_core::{AlbumId, ArtistId, Playlist, TrackId};
use mixtape_library::Db;
use mixtape_yt::search::{SearchItem, SearchKind, SearchPage};
use mixtape_yt::thumbs;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

/// Results kept; the one used longest ago goes first.
pub const CACHE_CAP: usize = 32;
/// A kept result is searched again after this long.
pub const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Thumbnails downloaded at once by one search.
pub const COVER_PARALLEL: usize = 8;

// ---------- the result cache ----------

/// A query as the cache knows it: outer spaces gone, inner runs of spaces folded, lower case.
pub fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// What a kept result is filed under: the normalized query and the filter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchKey {
    query: String,
    kind: SearchKind,
}

impl SearchKey {
    pub fn new(query: &str, kind: SearchKind) -> Self {
        Self {
            query: normalize_query(query),
            kind,
        }
    }
}

struct Entry<V> {
    value: V,
    stored: Instant,
    used: u64,
}

/// A bounded map whose entries expire. Time is passed in, so it can be tested without waiting.
pub struct TtlCache<K, V> {
    map: HashMap<K, Entry<V>>,
    cap: usize,
    ttl: Duration,
    tick: u64,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone> TtlCache<K, V> {
    pub fn new(cap: usize, ttl: Duration) -> Self {
        Self {
            map: HashMap::new(),
            cap,
            ttl,
            tick: 0,
        }
    }

    /// The value if it is younger than the TTL; a hit counts as use.
    pub fn get(&mut self, key: &K, now: Instant) -> Option<V> {
        let expired = {
            let e = self.map.get(key)?;
            now.saturating_duration_since(e.stored) >= self.ttl
        };
        if expired {
            self.map.remove(key);
            return None;
        }
        self.tick += 1;
        let e = self.map.get_mut(key)?;
        e.used = self.tick;
        Some(e.value.clone())
    }

    /// Keeps `value`; expired entries go first, then the least recently used.
    pub fn put(&mut self, key: K, value: V, now: Instant) {
        if self.cap == 0 {
            return;
        }
        let ttl = self.ttl;
        self.map
            .retain(|_, e| now.saturating_duration_since(e.stored) < ttl);
        self.tick += 1;
        if !self.map.contains_key(&key)
            && self.map.len() >= self.cap
            && let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
        {
            self.map.remove(&oldest);
        }
        self.map.insert(
            key,
            Entry {
                value,
                stored: now,
                used: self.tick,
            },
        );
    }

    pub fn remove(&mut self, key: &K) {
        self.map.remove(key);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------- cancelling a superseded search ----------

/// The search was replaced by a newer one while it ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superseded;

#[derive(Default)]
struct GateState {
    generation: u64,
    handles: Vec<AbortHandle>,
}

/// Keeps the tasks of the newest search; starting another aborts them.
#[derive(Clone, Default)]
pub struct SearchGate(Arc<Mutex<GateState>>);

impl SearchGate {
    /// Starts a new generation: everything tracked by the older ones is aborted.
    pub fn begin(&self) -> Ticket {
        let mut s = locked(&self.0);
        s.generation += 1;
        for h in s.handles.drain(..) {
            h.abort();
        }
        Ticket {
            gate: self.clone(),
            generation: s.generation,
        }
    }

    /// A ticket for the newest generation, without superseding it (the next page of results).
    pub fn current(&self) -> Ticket {
        Ticket {
            gate: self.clone(),
            generation: locked(&self.0).generation,
        }
    }
}

/// One search's right to keep running.
#[derive(Clone)]
pub struct Ticket {
    gate: SearchGate,
    generation: u64,
}

impl Ticket {
    pub fn is_current(&self) -> bool {
        locked(&self.gate.0).generation == self.generation
    }

    /// Tracks a task of this search; one that arrives after the search was superseded is
    /// aborted at once.
    pub fn track(&self, handle: AbortHandle) {
        let mut s = locked(&self.gate.0);
        if s.generation != self.generation {
            handle.abort();
            return;
        }
        s.handles.retain(|h| !h.is_finished());
        s.handles.push(handle);
    }

    /// Runs `fut` as a task of this search. A panic in it is passed on.
    pub async fn run<T, Fut>(&self, fut: Fut) -> Result<T, Superseded>
    where
        T: Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let task = tokio::spawn(fut);
        self.track(task.abort_handle());
        match task.await {
            Ok(v) => Ok(v),
            Err(e) if e.is_cancelled() => Err(Superseded),
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }
}

// ---------- fetching the pictures afterwards ----------

/// Where a thumbnail comes from; the apps use [`HttpCovers`], tests a fake.
pub trait CoverFetch: Clone + Send + Sync + 'static {
    /// Downloads and stores the small cover of `key`; its palette when it is stored.
    fn fetch(&self, key: &str, url: &str) -> impl Future<Output = Option<Vec<String>>> + Send;
}

/// Thumbnails from YouTube's image servers into the cover store.
#[derive(Clone)]
pub struct HttpCovers {
    http: reqwest::Client,
    store: CoverStore,
}

impl HttpCovers {
    pub fn new(http: reqwest::Client, store: CoverStore) -> Self {
        Self { http, store }
    }
}

impl CoverFetch for HttpCovers {
    async fn fetch(&self, key: &str, url: &str) -> Option<Vec<String>> {
        let bytes = thumbs::download_small(&self.http, url).await.ok()?;
        let (store, key) = (self.store.clone(), key.to_owned());
        tokio::task::spawn_blocking(move || {
            store
                .process(&key, &bytes, Detail::Small)
                .ok()
                .map(|p| p.palette)
        })
        .await
        .ok()
        .flatten()
    }
}

/// A cover to fetch: its key in the cover store and where the thumbnail is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverJob {
    pub key: String,
    pub url: String,
}

/// Everything the background fetch needs.
#[derive(Clone)]
pub struct CoverSink<F> {
    pub db: Db,
    pub store: CoverStore,
    pub fetcher: F,
    /// Called with the key of each cover that landed (or turned out to be there already).
    pub on_ready: Arc<dyn Fn(&str) + Send + Sync>,
}

/// Fetches the covers of `jobs` that are not on disk, in one background task with one limit
/// of [`COVER_PARALLEL`], in the order given. The task belongs to `ticket`: a newer search
/// aborts it. `None` when nothing is missing.
pub fn spawn_covers<F: CoverFetch>(
    ticket: &Ticket,
    sink: &CoverSink<F>,
    jobs: &[CoverJob],
) -> Option<JoinHandle<()>> {
    let todo: Vec<CoverJob> = jobs
        .iter()
        .filter(|j| !sink.store.exists(&j.key))
        .cloned()
        .collect();
    if todo.is_empty() {
        return None;
    }
    let task = tokio::spawn(fetch_all(sink.clone(), todo));
    ticket.track(task.abort_handle());
    Some(task)
}

async fn fetch_all<F: CoverFetch>(sink: CoverSink<F>, jobs: Vec<CoverJob>) {
    let sem = Arc::new(Semaphore::new(COVER_PARALLEL));
    let mut set = JoinSet::new();
    for job in jobs {
        // The permit is taken here, so the downloads start in the order of the screen.
        let Ok(permit) = sem.clone().acquire_owned().await else {
            return;
        };
        let sink = sink.clone();
        set.spawn(async move {
            let _permit = permit;
            if !sink.store.exists(&job.key) {
                let Some(palette) = sink.fetcher.fetch(&job.key, &job.url).await else {
                    return;
                };
                if !palette.is_empty() {
                    let (db, key) = (sink.db.clone(), job.key.clone());
                    let _ =
                        tokio::task::spawn_blocking(move || persist_palette(&db, &key, &palette))
                            .await;
                }
            }
            (sink.on_ready)(&job.key);
        });
    }
    while set.join_next().await.is_some() {}
}

/// A browsed album or artist is stored before its cover is there; the palette it gets from
/// the cover is filled in when the cover lands (only where none is stored yet).
fn persist_palette(db: &Db, key: &str, palette: &[String]) {
    let Ok(json) = serde_json::to_string(palette) else {
        return;
    };
    let (key, json) = (key.to_owned(), json);
    let res = db.write(move |t| {
        for table in ["albums", "artists"] {
            t.execute(
                &format!(
                    "UPDATE {table} SET palette = ?2 WHERE id = ?1 AND (palette IS NULL OR palette IN ('', '[]'))"
                ),
                rusqlite::params![key, json],
            )?;
        }
        Ok(())
    });
    if let Err(e) = res {
        tracing::warn!(error = %e, "storing a cover palette failed");
    }
}

// ---------- storing a page ----------

/// Which list of the page holds the top card before it is picked back out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopKind {
    Track,
    Album,
    Artist,
    Playlist,
}

/// A page whose rows are in the database: the ids to read back, in order.
#[derive(Debug, Clone)]
pub struct Stored {
    pub top: Option<TopKind>,
    pub tracks: Vec<TrackId>,
    pub albums: Vec<AlbumId>,
    pub artists: Vec<ArtistId>,
    pub playlists: Vec<Playlist>,
    pub continuation: Option<String>,
    /// Thumbnails of the page in the order of the screen (some may be on disk already).
    pub covers: Vec<CoverJob>,
}

/// Writes what the page found; no cover is downloaded here. Every row records the cover key
/// of its thumbnail at once and the cover is fetched afterwards ([`spawn_covers`]). Blocks on
/// the database: run it where that is fine.
pub fn store_page(
    db: &Db,
    covers: &CoverStore,
    page: SearchPage,
) -> mixtape_library::Result<Stored> {
    let mut tracks = page.tracks;
    let mut albums = page.albums;
    let mut artists = page.artists;
    let mut playlists = page.playlists;
    // The top card is stored with the rest, then picked back out.
    let top = page.top.map(|top| match top {
        SearchItem::Track(t) => {
            tracks.insert(0, t);
            TopKind::Track
        }
        SearchItem::Album(a) => {
            albums.insert(0, a);
            TopKind::Album
        }
        SearchItem::Artist(a) => {
            artists.insert(0, a);
            TopKind::Artist
        }
        SearchItem::Playlist(p) => {
            playlists.insert(0, p);
            TopKind::Playlist
        }
    });
    // The thumbnails in the order the screen shows them: the top card first.
    type Thumb = (String, Option<String>);
    let lists: [(TopKind, Vec<Thumb>); 4] = [
        (
            TopKind::Track,
            tracks
                .iter()
                .map(|t| (sources::cover_key(t), t.thumbnail.clone()))
                .collect(),
        ),
        (
            TopKind::Album,
            albums
                .iter()
                .map(|a| (a.id.to_string(), a.thumbnail.clone()))
                .collect(),
        ),
        (
            TopKind::Artist,
            artists
                .iter()
                .map(|a| (a.id.to_string(), a.thumbnail.clone()))
                .collect(),
        ),
        (
            TopKind::Playlist,
            playlists
                .iter()
                .map(|p| (p.id.to_string(), p.thumbnail.clone()))
                .collect(),
        ),
    ];
    let mut cards: Vec<Thumb> = Vec::new();
    // The top card heads its list: its picture goes first.
    if let Some(first) = top.and_then(|k| lists.iter().find(|(l, _)| *l == k)?.1.first().cloned()) {
        cards.push(first);
    }
    cards.extend(lists.into_iter().flat_map(|(_, l)| l));
    let mut seen = HashSet::new();
    let mut have = HashSet::new();
    let mut jobs = Vec::new();
    for (key, url) in cards {
        match url {
            Some(url) => {
                have.insert(key.clone());
                if seen.insert(key.clone()) {
                    jobs.push(CoverJob { key, url });
                }
            }
            None if covers.exists(&key) => {
                have.insert(key);
            }
            None => {}
        }
    }
    let tracks = sources::store_tracks_now(db, covers, tracks)?;
    let albums = catalog::write_album_cards(db, covers, &albums, &have);
    let artists = catalog::write_artist_cards(db, covers, &artists, &have);
    let playlists = catalog::playlist_rows(playlists, &have);
    Ok(Stored {
        top,
        tracks,
        albums,
        artists,
        playlists,
        continuation: page.continuation,
        covers: jobs,
    })
}

// ---------- the search itself ----------

/// Why a search did not give a page.
#[derive(Debug)]
pub enum SearchError<E> {
    /// A newer search replaced it.
    Superseded,
    /// YouTube did not answer.
    Read(E),
    /// The rows could not be written.
    Store(mixtape_library::Error),
    /// The storing task died.
    Internal(String),
}

type ResultCache = Arc<Mutex<TtlCache<SearchKey, Stored>>>;

/// The result cache and the gate of a running app.
#[derive(Clone)]
pub struct CatalogSearch {
    gate: SearchGate,
    cache: ResultCache,
}

impl Default for CatalogSearch {
    fn default() -> Self {
        Self::with_limits(CACHE_CAP, CACHE_TTL)
    }
}

impl CatalogSearch {
    pub fn with_limits(cap: usize, ttl: Duration) -> Self {
        Self {
            gate: SearchGate::default(),
            cache: Arc::new(Mutex::new(TtlCache::new(cap, ttl))),
        }
    }

    /// A new search: the one still running is aborted, with its pictures. A repeat of a
    /// query within the TTL is answered from the cache without asking YouTube; the pictures
    /// still missing are fetched either way. `read` asks YouTube.
    pub async fn search<F, E, Fut>(
        &self,
        key: SearchKey,
        sink: CoverSink<F>,
        read: impl FnOnce() -> Fut + Send + 'static,
    ) -> Result<Stored, SearchError<E>>
    where
        F: CoverFetch,
        E: Send + 'static,
        Fut: Future<Output = Result<SearchPage, E>> + Send + 'static,
    {
        let ticket = self.gate.begin();
        let cache = Some((self.cache.clone(), key));
        load(ticket, cache, sink, read).await
    }

    /// The next page of the current search: not cached, and a newer search aborts it.
    pub async fn more<F, E, Fut>(
        &self,
        sink: CoverSink<F>,
        read: impl FnOnce() -> Fut + Send + 'static,
    ) -> Result<Stored, SearchError<E>>
    where
        F: CoverFetch,
        E: Send + 'static,
        Fut: Future<Output = Result<SearchPage, E>> + Send + 'static,
    {
        load(self.gate.current(), None, sink, read).await
    }

    /// A search that is not the screen's (a recognized song): no cache, and no search of the
    /// screen aborts it.
    pub async fn lookup<F, E, Fut>(
        &self,
        sink: CoverSink<F>,
        read: impl FnOnce() -> Fut + Send + 'static,
    ) -> Result<Stored, SearchError<E>>
    where
        F: CoverFetch,
        E: Send + 'static,
        Fut: Future<Output = Result<SearchPage, E>> + Send + 'static,
    {
        load(SearchGate::default().begin(), None, sink, read).await
    }

    /// Drops a kept result whose rows are no longer all in the database.
    pub fn forget(&self, key: &SearchKey) {
        locked(&self.cache).remove(key);
    }

    /// Drops every kept result (the account changed).
    pub fn clear(&self) {
        *locked(&self.cache) = TtlCache::new(CACHE_CAP, CACHE_TTL);
    }
}

async fn load<F, E, Fut>(
    ticket: Ticket,
    cache: Option<(ResultCache, SearchKey)>,
    sink: CoverSink<F>,
    read: impl FnOnce() -> Fut + Send + 'static,
) -> Result<Stored, SearchError<E>>
where
    F: CoverFetch,
    E: Send + 'static,
    Fut: Future<Output = Result<SearchPage, E>> + Send + 'static,
{
    let inner = ticket.clone();
    let task = ticket.run(async move {
        let kept = cache
            .as_ref()
            .and_then(|(c, k)| locked(c).get(k, Instant::now()));
        let stored = match kept {
            Some(s) => s,
            None => {
                let page = read().await.map_err(SearchError::Read)?;
                let (db, store) = (sink.db.clone(), sink.store.clone());
                let stored = tokio::task::spawn_blocking(move || store_page(&db, &store, page))
                    .await
                    .map_err(|e| SearchError::Internal(e.to_string()))?
                    .map_err(SearchError::Store)?;
                if let Some((c, k)) = cache {
                    locked(&c).put(k, stored.clone(), Instant::now());
                }
                stored
            }
        };
        // Missing pictures are fetched on a hit too: the search that stored the rows may
        // have been aborted before they landed.
        spawn_covers(&inner, &sink, &stored.covers);
        Ok(stored)
    });
    task.await.map_err(|Superseded| SearchError::Superseded)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_core::{AlbumRef, ArtistRef, PlaylistId};
    use mixtape_library::repo;
    use mixtape_yt::api::{VideoKind, YtAlbumCard, YtArtistCard, YtPlaylistCard, YtTrack};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    // ----- normalization and the cache -----

    #[test]
    fn queries_differing_in_case_and_spaces_share_a_key() {
        let a = SearchKey::new("  Daft  Punk ", SearchKind::All);
        assert_eq!(a, SearchKey::new("daft punk", SearchKind::All));
        assert_eq!(normalize_query("\tMØ \n"), "mø");
        assert_ne!(a, SearchKey::new("daft punk", SearchKind::Tracks));
        assert_ne!(a, SearchKey::new("daft punks", SearchKind::All));
    }

    #[test]
    fn cache_hit_and_miss() {
        let now = Instant::now();
        let mut c = TtlCache::new(4, Duration::from_secs(60));
        assert_eq!(c.get(&"a", now), None);
        c.put("a", 1, now);
        assert_eq!(c.get(&"a", at(now, 59)), Some(1));
        assert_eq!(c.get(&"b", now), None);
    }

    #[test]
    fn cache_entry_expires_exactly_at_the_ttl() {
        let now = Instant::now();
        let mut c = TtlCache::new(4, Duration::from_secs(60));
        c.put("a", 1, now);
        assert_eq!(c.get(&"a", at(now, 60)), None);
        // An expired entry is gone for good, not just hidden.
        assert!(c.is_empty());
    }

    #[test]
    fn a_hit_does_not_extend_the_ttl() {
        let now = Instant::now();
        let mut c = TtlCache::new(4, Duration::from_secs(60));
        c.put("a", 1, now);
        assert_eq!(c.get(&"a", at(now, 50)), Some(1));
        assert_eq!(c.get(&"a", at(now, 61)), None);
    }

    #[test]
    fn putting_again_replaces_and_restarts_the_clock() {
        let now = Instant::now();
        let mut c = TtlCache::new(4, Duration::from_secs(60));
        c.put("a", 1, now);
        c.put("a", 2, at(now, 50));
        assert_eq!(c.get(&"a", at(now, 100)), Some(2));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn the_least_recently_used_entry_is_evicted() {
        let now = Instant::now();
        let mut c = TtlCache::new(2, Duration::from_secs(600));
        c.put("a", 1, now);
        c.put("b", 2, now);
        // `a` is used, so `b` is the one to go.
        assert_eq!(c.get(&"a", now), Some(1));
        c.put("c", 3, now);
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(&"b", now), None);
        assert_eq!(c.get(&"a", now), Some(1));
        assert_eq!(c.get(&"c", now), Some(3));
    }

    #[test]
    fn replacing_a_key_at_capacity_evicts_nothing() {
        let now = Instant::now();
        let mut c = TtlCache::new(2, Duration::from_secs(600));
        c.put("a", 1, now);
        c.put("b", 2, now);
        c.put("a", 9, now);
        assert_eq!(c.get(&"b", now), Some(2));
        assert_eq!(c.get(&"a", now), Some(9));
    }

    #[test]
    fn expired_entries_make_room_before_live_ones_are_evicted() {
        let now = Instant::now();
        let mut c = TtlCache::new(2, Duration::from_secs(60));
        c.put("old", 1, now);
        c.put("live", 2, at(now, 50));
        c.put("new", 3, at(now, 70));
        assert_eq!(c.get(&"live", at(now, 70)), Some(2));
        assert_eq!(c.get(&"new", at(now, 70)), Some(3));
    }

    #[test]
    fn the_bound_holds_under_many_puts() {
        let now = Instant::now();
        let mut c = TtlCache::new(CACHE_CAP, CACHE_TTL);
        for i in 0..200 {
            c.put(i, i, now);
            assert!(c.len() <= CACHE_CAP);
        }
        assert_eq!(c.get(&199, now), Some(199));
        assert_eq!(c.get(&0, now), None);
    }

    #[test]
    fn a_zero_capacity_cache_keeps_nothing() {
        let now = Instant::now();
        let mut c = TtlCache::new(0, Duration::from_secs(60));
        c.put("a", 1, now);
        assert_eq!(c.get(&"a", now), None);
    }

    // ----- the gate -----

    #[tokio::test]
    async fn a_newer_search_aborts_the_running_one() {
        let gate = SearchGate::default();
        let first = gate.begin();
        let started = Arc::new(Notify::new());
        let s = started.clone();
        let running = tokio::spawn(async move {
            first
                .run(async move {
                    s.notify_one();
                    std::future::pending::<()>().await;
                })
                .await
        });
        started.notified().await;
        let second = gate.begin();
        assert_eq!(running.await.unwrap(), Err(Superseded));
        assert_eq!(second.run(async { 7 }).await, Ok(7));
    }

    #[tokio::test]
    async fn the_dropped_search_stops_running() {
        struct Flag(Arc<AtomicUsize>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let gate = SearchGate::default();
        let dropped = Arc::new(AtomicUsize::new(0));
        let first = gate.begin();
        let flag = Flag(dropped.clone());
        let started = Arc::new(Notify::new());
        let s = started.clone();
        let task = tokio::spawn(async move {
            let _ = first
                .run(async move {
                    let _flag = flag;
                    s.notify_one();
                    std::future::pending::<()>().await;
                })
                .await;
        });
        started.notified().await;
        gate.begin();
        task.await.unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_task_tracked_after_the_search_was_superseded_is_aborted() {
        let gate = SearchGate::default();
        let old = gate.begin();
        gate.begin();
        let late = tokio::spawn(std::future::pending::<()>());
        old.track(late.abort_handle());
        assert!(late.await.unwrap_err().is_cancelled());
        assert!(!old.is_current());
    }

    #[tokio::test]
    async fn the_next_page_is_aborted_by_a_new_search_but_does_not_abort_the_search() {
        let gate = SearchGate::default();
        let search = gate.begin();
        let covers = tokio::spawn(std::future::pending::<()>());
        search.track(covers.abort_handle());
        // The next page joins the generation without ending it.
        let page = gate.current();
        assert!(page.is_current() && search.is_current());
        assert!(!covers.is_finished());
        let more = tokio::spawn(std::future::pending::<()>());
        page.track(more.abort_handle());
        gate.begin();
        assert!(covers.await.unwrap_err().is_cancelled());
        assert!(more.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    #[should_panic(expected = "boom")]
    async fn a_panic_in_a_search_is_not_taken_for_a_cancel() {
        let gate = SearchGate::default();
        let _ = gate.begin().run(async { panic!("boom") }).await;
    }

    // ----- the pipeline -----

    fn track(id: &str, album: Option<&str>, thumb: Option<&str>) -> YtTrack {
        YtTrack {
            id: TrackId::new(id),
            title: format!("song {id}"),
            artists: vec![ArtistRef {
                id: None,
                name: "Artist".into(),
            }],
            album: album.map(|a| AlbumRef {
                id: AlbumId::new(a),
                title: format!("album {a}"),
            }),
            duration_ms: 1000,
            thumbnail: thumb.map(str::to_owned),
            kind: VideoKind::Audio,
            track_no: None,
        }
    }

    fn album_card(id: &str, thumb: Option<&str>) -> YtAlbumCard {
        YtAlbumCard {
            id: AlbumId::new(id),
            title: format!("album {id}"),
            artists: Vec::new(),
            year: None,
            album_type: None,
            thumbnail: thumb.map(str::to_owned),
        }
    }

    fn artist_card(id: &str, thumb: Option<&str>) -> YtArtistCard {
        YtArtistCard {
            id: ArtistId::new(id),
            name: format!("artist {id}"),
            thumbnail: thumb.map(str::to_owned),
        }
    }

    fn playlist_card(id: &str, thumb: Option<&str>) -> YtPlaylistCard {
        YtPlaylistCard {
            id: PlaylistId::new(id),
            title: format!("list {id}"),
            author: None,
            track_count: Some(3),
            thumbnail: thumb.map(str::to_owned),
        }
    }

    fn page() -> SearchPage {
        SearchPage {
            top: Some(SearchItem::Album(album_card(
                "MPREb_top",
                Some("http://t/top"),
            ))),
            tracks: vec![
                track("t1", Some("MPREb_a"), Some("http://t/a")),
                track("t2", None, Some("http://t/t2")),
                track("t3", None, None),
            ],
            albums: vec![album_card("MPREb_b", Some("http://t/b"))],
            artists: vec![artist_card("UCart", Some("http://t/art"))],
            playlists: vec![playlist_card("VLpl", Some("http://t/pl"))],
            continuation: Some("next".into()),
        }
    }

    fn db_and_store() -> (tempfile::TempDir, Db, CoverStore) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let store = CoverStore::new(dir.path().join("covers"));
        (dir, db, store)
    }

    #[test]
    fn a_page_is_stored_with_cover_keys_before_any_cover_exists() {
        let (_dir, db, store) = db_and_store();
        let s = store_page(&db, &store, page()).unwrap();
        assert_eq!(s.top, Some(TopKind::Album));
        // The album's top card heads the list; the order of the page is kept.
        assert_eq!(s.albums[0].as_str(), "MPREb_top");
        assert_eq!(s.tracks.len(), 3);
        assert_eq!(s.continuation.as_deref(), Some("next"));
        // Ready covers none, yet rows know their key (the tile draws once it lands).
        assert_eq!(s.playlists[0].cover_key.as_deref(), Some("VLpl"));
        let (tracks, albums, artists) = db
            .read(|c| {
                Ok((
                    repo::tracks_by_ids(c, &s.tracks)?,
                    repo::albums_by_ids(c, &s.albums)?,
                    repo::artists_by_ids(c, &s.artists)?,
                ))
            })
            .unwrap();
        assert_eq!(tracks[0].cover_key.as_deref(), Some("MPREb_a"));
        assert_eq!(tracks[1].cover_key.as_deref(), Some("t2"));
        assert_eq!(tracks[2].cover_key, None);
        assert_eq!(albums[0].cover_key.as_deref(), Some("MPREb_top"));
        assert_eq!(artists[0].image_key.as_deref(), Some("UCart"));
    }

    #[test]
    fn cover_jobs_follow_the_screen_order_without_repeats() {
        let (_dir, db, store) = db_and_store();
        let s = store_page(&db, &store, page()).unwrap();
        let keys: Vec<&str> = s.covers.iter().map(|j| j.key.as_str()).collect();
        // The top card first, then tracks, albums, artists, playlists.
        assert_eq!(
            keys,
            ["MPREb_top", "MPREb_a", "t2", "MPREb_b", "UCart", "VLpl"]
        );
        // Two tracks of one album share the album's cover: one job.
        let mut p = page();
        p.tracks = vec![
            track("t1", Some("MPREb_a"), Some("http://t/a")),
            track("t2", Some("MPREb_a"), Some("http://t/a")),
        ];
        p.top = None;
        let s = store_page(&db, &store, p).unwrap();
        assert_eq!(s.covers.iter().filter(|j| j.key == "MPREb_a").count(), 1);
    }

    #[test]
    fn a_top_track_leads_the_jobs_and_a_page_without_a_top_keeps_the_default_order() {
        let (_dir, db, store) = db_and_store();
        let mut p = page();
        p.top = Some(SearchItem::Artist(artist_card("UCtop", Some("http://t/x"))));
        let s = store_page(&db, &store, p).unwrap();
        assert_eq!(s.top, Some(TopKind::Artist));
        assert_eq!(s.covers[0].key, "UCtop");
        let mut p = page();
        p.top = None;
        let s = store_page(&db, &store, p).unwrap();
        assert_eq!(s.top, None);
        assert_eq!(s.covers[0].key, "MPREb_a");
    }

    #[test]
    fn an_empty_page_stores_nothing_and_has_no_jobs() {
        let (_dir, db, store) = db_and_store();
        let s = store_page(&db, &store, SearchPage::default()).unwrap();
        assert!(s.tracks.is_empty() && s.albums.is_empty() && s.covers.is_empty());
        assert_eq!(s.top, None);
    }

    #[test]
    fn a_cover_already_on_disk_is_not_a_job_but_still_recorded() {
        let (_dir, db, store) = db_and_store();
        // No thumbnail URL, but the cover is in the store from an earlier visit.
        put_cover(&store, "UCart");
        put_cover(&store, "MPREb_b");
        let p = SearchPage {
            artists: vec![artist_card("UCart", None)],
            albums: vec![album_card("MPREb_b", None)],
            ..Default::default()
        };
        let s = store_page(&db, &store, p).unwrap();
        assert!(s.covers.is_empty());
        let albums = db.read(|c| repo::albums_by_ids(c, &s.albums)).unwrap();
        assert_eq!(albums[0].cover_key.as_deref(), Some("MPREb_b"));
    }

    /// A cover on disk without decoding anything: `exists` only looks for the file.
    fn put_cover(store: &CoverStore, key: &str) {
        let path = store.path(key, 300).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"jpeg").unwrap();
    }

    /// Covers that wait until the test lets them through.
    #[derive(Clone)]
    struct SlowCovers {
        gate: Arc<tokio::sync::Semaphore>,
        started: Arc<AtomicUsize>,
        palette: Vec<String>,
    }

    impl SlowCovers {
        fn new() -> Self {
            Self {
                gate: Arc::new(tokio::sync::Semaphore::new(0)),
                started: Arc::new(AtomicUsize::new(0)),
                palette: vec!["#112233".into()],
            }
        }

        /// Lets every cover through, now and later.
        fn open(&self) {
            self.gate.add_permits(10_000);
        }

        /// Waits until `n` fetches have started.
        async fn started_at_least(&self, n: usize) {
            tokio::time::timeout(Duration::from_secs(5), async {
                while self.started.load(Ordering::SeqCst) < n {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("covers did not start");
        }
    }

    impl CoverFetch for SlowCovers {
        async fn fetch(&self, _key: &str, _url: &str) -> Option<Vec<String>> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.gate.acquire().await.ok()?.forget();
            Some(self.palette.clone())
        }
    }

    fn sink(
        db: &Db,
        store: &CoverStore,
        fetcher: SlowCovers,
    ) -> (
        CoverSink<SlowCovers>,
        tokio::sync::mpsc::UnboundedReceiver<String>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = CoverSink {
            db: db.clone(),
            store: store.clone(),
            fetcher,
            on_ready: Arc::new(move |k| {
                let _ = tx.send(k.to_owned());
            }),
        };
        (sink, rx)
    }

    async fn ok_page() -> Result<SearchPage, ()> {
        Ok(page())
    }

    #[tokio::test]
    async fn the_answer_does_not_wait_for_the_covers() {
        let (_dir, db, store) = db_and_store();
        let slow = SlowCovers::new();
        let (sink, mut ready) = sink(&db, &store, slow.clone());
        let cs = CatalogSearch::default();
        // The covers stay parked; the search must still come back.
        let stored = tokio::time::timeout(
            Duration::from_secs(5),
            cs.search(SearchKey::new("q", SearchKind::All), sink, ok_page),
        )
        .await
        .expect("search waited for the covers")
        .unwrap();
        assert_eq!(stored.tracks.len(), 3);
        assert!(ready.try_recv().is_err());
        // Letting the covers through announces each one.
        let jobs = stored.covers.len();
        slow.started_at_least(1).await;
        slow.open();
        let mut got = HashSet::new();
        while got.len() < jobs {
            got.insert(
                tokio::time::timeout(Duration::from_secs(5), ready.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        assert!(got.contains("MPREb_top") && got.contains("VLpl"));
    }

    #[tokio::test]
    async fn a_landed_cover_gives_its_palette_to_the_stored_album_and_artist() {
        let (_dir, db, store) = db_and_store();
        let slow = SlowCovers::new();
        let (sink, mut ready) = sink(&db, &store, slow.clone());
        let cs = CatalogSearch::default();
        let stored = cs
            .search(SearchKey::new("q", SearchKind::All), sink, ok_page)
            .await
            .unwrap();
        slow.open();
        for _ in 0..stored.covers.len() {
            tokio::time::timeout(Duration::from_secs(5), ready.recv())
                .await
                .unwrap();
        }
        let (albums, artists) = db
            .read(|c| {
                Ok((
                    repo::albums_by_ids(c, &stored.albums)?,
                    repo::artists_by_ids(c, &stored.artists)?,
                ))
            })
            .unwrap();
        assert_eq!(albums[0].palette, vec!["#112233".to_owned()]);
        assert_eq!(artists[0].palette, vec!["#112233".to_owned()]);
    }

    #[tokio::test]
    async fn a_failed_cover_is_not_announced() {
        #[derive(Clone)]
        struct Broken;
        impl CoverFetch for Broken {
            async fn fetch(&self, _: &str, _: &str) -> Option<Vec<String>> {
                None
            }
        }
        let (_dir, db, store) = db_and_store();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let sink = CoverSink {
            db,
            store,
            fetcher: Broken,
            on_ready: Arc::new(move |k| {
                let _ = tx.send(k.to_owned());
            }),
        };
        let ticket = SearchGate::default().begin();
        let jobs = vec![CoverJob {
            key: "k".into(),
            url: "u".into(),
        }];
        spawn_covers(&ticket, &sink, &jobs).unwrap().await.unwrap();
        drop(sink);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn covers_start_in_order_and_never_more_than_the_limit_at_once() {
        #[derive(Clone)]
        struct Counting {
            now: Arc<AtomicUsize>,
            peak: Arc<AtomicUsize>,
            order: Arc<Mutex<Vec<String>>>,
        }
        impl CoverFetch for Counting {
            async fn fetch(&self, key: &str, _: &str) -> Option<Vec<String>> {
                locked(&self.order).push(key.to_owned());
                let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                self.now.fetch_sub(1, Ordering::SeqCst);
                Some(Vec::new())
            }
        }
        let (_dir, db, store) = db_and_store();
        let c = Counting {
            now: Arc::default(),
            peak: Arc::default(),
            order: Arc::default(),
        };
        let sink = CoverSink {
            db,
            store,
            fetcher: c.clone(),
            on_ready: Arc::new(|_| {}),
        };
        let jobs: Vec<CoverJob> = (0..40)
            .map(|i| CoverJob {
                key: format!("k{i}"),
                url: "u".into(),
            })
            .collect();
        let ticket = SearchGate::default().begin();
        spawn_covers(&ticket, &sink, &jobs).unwrap().await.unwrap();
        assert!(c.peak.load(Ordering::SeqCst) <= COVER_PARALLEL);
        assert!(c.peak.load(Ordering::SeqCst) > 1);
        let order = locked(&c.order).clone();
        assert_eq!(order.len(), 40);
        assert_eq!(&order[0], "k0");
    }

    #[tokio::test]
    async fn a_repeat_of_the_query_is_answered_from_the_cache() {
        let (_dir, db, store) = db_and_store();
        let (sink, _ready) = sink(&db, &store, SlowCovers::new());
        let cs = CatalogSearch::default();
        let reads = Arc::new(AtomicUsize::new(0));
        let read = |reads: Arc<AtomicUsize>| {
            move || async move {
                reads.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(page())
            }
        };
        let a = cs
            .search(
                SearchKey::new("Daft Punk", SearchKind::All),
                sink.clone(),
                read(reads.clone()),
            )
            .await
            .unwrap();
        let b = cs
            .search(
                SearchKey::new(" daft  punk ", SearchKind::All),
                sink.clone(),
                read(reads.clone()),
            )
            .await
            .unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(a.tracks, b.tracks);
        // Another filter is another result.
        cs.search(
            SearchKey::new("daft punk", SearchKind::Tracks),
            sink.clone(),
            read(reads.clone()),
        )
        .await
        .unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        // A forgotten result is read again.
        cs.forget(&SearchKey::new("daft punk", SearchKind::All));
        cs.search(
            SearchKey::new("daft punk", SearchKind::All),
            sink.clone(),
            read(reads.clone()),
        )
        .await
        .unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn an_expired_result_is_read_again() {
        let (_dir, db, store) = db_and_store();
        let (sink, _ready) = sink(&db, &store, SlowCovers::new());
        let cs = CatalogSearch::with_limits(8, Duration::from_millis(30));
        let reads = Arc::new(AtomicUsize::new(0));
        let search = |cs: &CatalogSearch, sink: CoverSink<SlowCovers>, reads: Arc<AtomicUsize>| {
            let cs = cs.clone();
            async move {
                cs.search(
                    SearchKey::new("q", SearchKind::All),
                    sink,
                    move || async move {
                        reads.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ()>(page())
                    },
                )
                .await
                .unwrap()
            }
        };
        search(&cs, sink.clone(), reads.clone()).await;
        search(&cs, sink.clone(), reads.clone()).await;
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(60)).await;
        search(&cs, sink.clone(), reads.clone()).await;
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_read_is_not_cached() {
        let (_dir, db, store) = db_and_store();
        let (sink, _ready) = sink(&db, &store, SlowCovers::new());
        let cs = CatalogSearch::default();
        let key = SearchKey::new("q", SearchKind::All);
        let r = cs
            .search(key.clone(), sink.clone(), || async {
                Err::<SearchPage, _>("down")
            })
            .await;
        assert!(matches!(r, Err(SearchError::Read("down"))));
        let r = cs.search(key, sink, ok_page).await;
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn a_cache_hit_fetches_the_covers_still_missing() {
        let (_dir, db, store) = db_and_store();
        let slow = SlowCovers::new();
        let (sink, _ready) = sink(&db, &store, slow.clone());
        let cs = CatalogSearch::default();
        let key = SearchKey::new("q", SearchKind::All);
        let first = cs.search(key.clone(), sink.clone(), ok_page).await.unwrap();
        slow.started_at_least(first.covers.len()).await;
        let before = slow.started.load(Ordering::SeqCst);
        // A newer search (with no covers of its own) aborts the covers of the first one...
        cs.search(
            SearchKey::new("other", SearchKind::All),
            sink.clone(),
            || async { Ok::<_, ()>(SearchPage::default()) },
        )
        .await
        .unwrap();
        // ... so the repeat of the first must start them again, from the cache.
        let reads = Arc::new(AtomicUsize::new(0));
        let r = reads.clone();
        let again = cs
            .search(key, sink, move || async move {
                r.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(page())
            })
            .await
            .unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert_eq!(first.covers, again.covers);
        slow.started_at_least(before + again.covers.len()).await;
    }

    #[tokio::test]
    async fn a_newer_search_cancels_the_older_one_and_its_covers() {
        let (_dir, db, store) = db_and_store();
        let slow = SlowCovers::new();
        let (sink, mut ready) = sink(&db, &store, slow.clone());
        let cs = CatalogSearch::default();
        // The first search's read never answers.
        let first = {
            let (cs, sink) = (cs.clone(), sink.clone());
            tokio::spawn(async move {
                cs.search(SearchKey::new("a", SearchKind::All), sink, || async {
                    std::future::pending::<Result<SearchPage, ()>>().await
                })
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let second = cs
            .search(SearchKey::new("ab", SearchKind::All), sink.clone(), ok_page)
            .await
            .unwrap();
        assert!(matches!(first.await.unwrap(), Err(SearchError::Superseded)));
        // The second search's covers go on...
        slow.started_at_least(1).await;
        // ... until a third replaces it: then nothing more is announced.
        let third = {
            let (cs, sink) = (cs.clone(), sink.clone());
            tokio::spawn(async move {
                cs.search(SearchKey::new("abc", SearchKind::Albums), sink, || async {
                    std::future::pending::<Result<SearchPage, ()>>().await
                })
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!second.covers.is_empty());
        slow.open();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            ready.try_recv().is_err(),
            "an aborted search announced a cover"
        );
        third.abort();
    }

    #[tokio::test]
    async fn a_lookup_is_not_aborted_by_a_search() {
        let (_dir, db, store) = db_and_store();
        let (sink, _ready) = sink(&db, &store, SlowCovers::new());
        let cs = CatalogSearch::default();
        let started = Arc::new(Notify::new());
        let (s, go) = (started.clone(), Arc::new(Notify::new()));
        let go2 = go.clone();
        let lookup = {
            let (cs, sink) = (cs.clone(), sink.clone());
            tokio::spawn(async move {
                cs.lookup(sink, move || async move {
                    s.notify_one();
                    go2.notified().await;
                    Ok::<_, ()>(page())
                })
                .await
            })
        };
        started.notified().await;
        cs.search(SearchKey::new("x", SearchKind::All), sink, ok_page)
            .await
            .unwrap();
        go.notify_one();
        assert!(lookup.await.unwrap().is_ok());
    }
}
