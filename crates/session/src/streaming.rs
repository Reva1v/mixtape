//! Streaming for tracks without audio on disk (US3): resolves the URL, fills a sparse
//! cache file from the network while the engine plays it, moves the finished file into
//! `cache/`, measures its loudness and keeps the cache under its limit. This is the
//! player's [`StreamProvider`].
//!
//! Network use is kept low: URLs of the upcoming tracks are resolved in one yt-dlp run once
//! fewer than [`RESOLVE_LOW_WATER`] are left, and the next track is downloaded only after
//! the current one's download has ended (or right away when the current track is on disk).
//!
//! Loudness for normalisation: YouTube's value comes from one `player` call per track (signed
//! in only), sent for the current and the upcoming tracks alongside their yt-dlp runs (it
//! answers much sooner), so it is usually stored before the first sample plays. A late
//! answer, and the R128 measurement of a finished download, make the player glide to the new
//! gain.

use mixtape_audio::{SparseCacheFile, Source};
use mixtape_core::{AppError, TrackId, tr, trf};
use mixtape_library::{Db, cache_lru, repo};
use mixtape_player::prefetch::{CACHE_AHEAD, RESOLVE_LOW_WATER};
use mixtape_player::{Opened, Player, StreamProvider};
use mixtape_sync::fetch;
use mixtape_sync::stream::StreamFill;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::resolver::{Container, StreamResolver};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use tauri::{AppHandle, Manager};
use tokio::sync::{Notify, OnceCell};

/// A download in progress (or just finished) for one track.
struct Active {
    file: Arc<SparseCacheFile>,
    container: Container,
    itag: u32,
    fill: StreamFill,
}

type Slot = Arc<OnceCell<Arc<Active>>>;

pub struct AppStreams {
    me: Weak<AppStreams>,
    app: AppHandle,
    rt: tokio::runtime::Handle,
    db: Db,
    root: PathBuf,
    resolver: Arc<StreamResolver>,
    http: mixtape_sync::reqwest::Client,
    yt: Arc<InnerTube>,
    /// Told when a track's loudness becomes known; set once the player exists.
    player: OnceLock<Weak<Player>>,
    /// Tracks whose YouTube loudness was asked for this session (asked once, even if absent).
    loudness_asked: Mutex<HashSet<TrackId>>,
    active: Mutex<HashMap<TrackId, Slot>>,
    /// The current track and the ones after it: kept from eviction and cancellation.
    wanted: Mutex<Vec<TrackId>>,
    /// The track playing now, as of the last prefetch.
    current: Mutex<Option<TrackId>>,
    /// Bumped by every prefetch: work started by an older one stops waiting.
    generation: AtomicU64,
    /// Woken when the wanted tracks change or a download ends.
    changed: Notify,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppStreams {
    pub fn new(
        app: AppHandle,
        rt: tokio::runtime::Handle,
        db: Db,
        root: PathBuf,
        resolver: Arc<StreamResolver>,
        yt: Arc<InnerTube>,
    ) -> Arc<Self> {
        let http = yt.http().clone();
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            app,
            rt,
            db,
            root,
            resolver,
            http,
            yt,
            player: OnceLock::new(),
            loudness_asked: Mutex::new(HashSet::new()),
            active: Mutex::new(HashMap::new()),
            wanted: Mutex::new(Vec::new()),
            current: Mutex::new(None),
            generation: AtomicU64::new(0),
            changed: Notify::new(),
        })
    }

    pub fn set_player(&self, player: &Arc<Player>) {
        let _ = self.player.set(Arc::downgrade(player));
    }

    fn refresh_gain(&self, id: &TrackId) {
        if let Some(p) = self.player.get().and_then(Weak::upgrade) {
            p.refresh_gain(id);
        }
    }

    /// Whether YouTube should be asked for `id`'s loudness: signed in (a guest gets no player
    /// config), nothing known about it yet, and not asked before in this session.
    fn wants_loudness(&self, id: &TrackId) -> bool {
        if !self.yt.is_authenticated() || lock(&self.loudness_asked).contains(id) {
            return false;
        }
        let known = self.db.read(|c| repo::track_loudness(c, id)).map(|l| l.lufs.is_some());
        !known.unwrap_or(true) && lock(&self.loudness_asked).insert(id.clone())
    }

    /// Asks YouTube for the loudness of each of `ids` that lacks it, one after another, stores
    /// it and lets the player apply it.
    async fn fetch_loudness(&self, ids: Vec<TrackId>) {
        for id in ids.into_iter().filter(|id| self.wants_loudness(id)) {
            match self.yt.loudness(&id).await {
                Ok(Some(lufs)) => {
                    let id2 = id.clone();
                    if self.db.write(move |t| repo::set_track_loudness(t, &id2, lufs)).is_ok() {
                        tracing::debug!(%id, lufs, "loudness from YouTube");
                        self.refresh_gain(&id);
                    }
                }
                Ok(None) => tracing::debug!(%id, "YouTube sent no loudness"),
                Err(e) => tracing::debug!(%id, error = %e, "loudness request failed"),
            }
        }
    }

    fn part_path(&self, id: &TrackId, c: Container) -> PathBuf {
        self.root.join("cache").join(format!("{}.{}.part", id.as_str(), c.as_str()))
    }

    fn rel_final(id: &TrackId, c: Container) -> String {
        format!("cache/{}.{}", id.as_str(), c.as_str())
    }

    fn has_audio(&self, id: &TrackId) -> bool {
        self.db.read(|c| repo::media_file(c, id)).ok().flatten().is_some()
    }

    /// Whether a download of `id` may start now: it is the current track, or the current
    /// track no longer needs the network (it is on disk, or its download has ended).
    fn may_download(&self, id: &TrackId) -> bool {
        let Some(current) = lock(&self.current).clone() else { return true };
        if &current == id {
            return true;
        }
        let slot = lock(&self.active).get(&current).cloned();
        match slot {
            // Still resolving, or downloading.
            Some(slot) => slot.get().is_some_and(|a| a.fill.done() || a.file.is_complete()),
            // Not started yet, or finished and moved into the cache.
            None => self.has_audio(&current),
        }
    }

    /// Waits until `id` may download. Gives up (`false`) once a newer prefetch came — with
    /// `strict` at once, else only when it no longer wants `id`.
    async fn wait_turn(&self, id: &TrackId, generation: u64, strict: bool) -> bool {
        loop {
            let notified = self.changed.notified();
            let moved_on = self.generation.load(Ordering::SeqCst) != generation;
            if moved_on && (strict || !lock(&self.wanted).contains(id)) {
                return false;
            }
            if self.may_download(id) {
                return true;
            }
            notified.await;
        }
    }

    /// The download for `id`, started on first use; concurrent callers share it.
    async fn start(&self, id: &TrackId) -> Result<Arc<Active>, String> {
        let slot = lock(&self.active).entry(id.clone()).or_default().clone();
        let res = slot.get_or_try_init(|| self.begin(id)).await.cloned();
        if res.is_err() {
            let mut active = lock(&self.active);
            if active.get(id).is_some_and(|s| Arc::ptr_eq(s, &slot)) {
                active.remove(id);
            }
        }
        res
    }

    async fn begin(&self, id: &TrackId) -> Result<Arc<Active>, String> {
        // The id becomes a file name under `cache/`: never let a stray one escape it.
        if !id.is_safe() {
            return Err(format!("invalid track id {:?}", id.as_str()));
        }
        let reason = |e: mixtape_yt::YtError| AppError::from(e).message;
        let mut stream = self.resolver.resolve_one(id).await.map_err(reason)?;
        let len = match fetch::exact_len(&self.http, &stream.url).await {
            Ok(n) => n,
            Err(fetch::FetchError::Expired) => {
                self.resolver.invalidate(id);
                stream = self.resolver.resolve_one(id).await.map_err(reason)?;
                fetch::exact_len(&self.http, &stream.url).await.map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e.to_string()),
        };
        let file = SparseCacheFile::create(self.part_path(id, stream.container), len)
            .map_err(|e| {
                trf!("Couldn't create the cache file: {e}", "Не вдалося створити файл кешу: {e}", "Не удалось создать файл кэша: {e}")
            })?;
        let me = self.me.clone();
        let id2 = id.clone();
        let fill = StreamFill::start(self.http.clone(), stream.url.clone(), file.clone(), move || {
            if let Some(s) = me.upgrade() {
                s.refresh_url(id2.clone());
            }
        });
        let active = Arc::new(Active { file, container: stream.container, itag: stream.itag, fill });
        if let Some(s) = self.me.upgrade() {
            let (id, active) = (id.clone(), active.clone());
            self.rt.spawn(async move { s.finish(id, active).await });
        }
        Ok(active)
    }

    /// googlevideo answered 403: resolve a fresh URL and point the download at it. When no
    /// URL comes, the download ends with the reason instead of waiting forever.
    fn refresh_url(&self, id: TrackId) {
        let Some(s) = self.me.upgrade() else { return };
        self.rt.spawn(async move {
            s.resolver.invalidate(&id);
            let res = s.resolver.resolve_one(&id).await;
            let Some(a) = lock(&s.active).get(&id).and_then(|slot| slot.get().cloned()) else { return };
            match res {
                Ok(stream) => a.fill.retarget(stream.url),
                Err(e) => {
                    a.file.fail(AppError::from(e).message);
                    a.fill.cancel();
                }
            }
        });
    }

    /// Waits for the download to end; a complete file moves into `cache/` and gets its loudness.
    async fn finish(self: Arc<Self>, id: TrackId, a: Arc<Active>) {
        let end = a.fill.ended().await;
        // The next track may start downloading now.
        self.changed.notify_waiters();
        if !a.file.is_complete() {
            // Failed, or stopped without a fresh URL: forget it so the next request starts
            // afresh. (A download dropped as unwanted is gone from the map already.)
            tracing::debug!(%id, ?end, "stream download ended incomplete");
            let mut active = lock(&self.active);
            if active.get(&id).and_then(|s| s.get()).is_some_and(|x| Arc::ptr_eq(x, &a)) {
                active.remove(&id);
            }
            return;
        }
        let rel = Self::rel_final(&id, a.container);
        let path = self.root.join(&rel);
        if let Err(e) = a.file.finalize(&path) {
            tracing::warn!(%id, error = %e, "finalizing the stream cache failed");
            return;
        }
        let s = self.clone();
        let _ = tokio::task::spawn_blocking(move || s.store(&id, &a, &rel, &path)).await;
    }

    fn store(&self, id: &TrackId, a: &Active, rel: &str, path: &std::path::Path) {
        let size = a.file.total_len();
        let (id2, rel2, container, itag) = (id.clone(), rel.to_owned(), a.container, a.itag);
        match self.db.write(move |t| cache_lru::register_cached(t, &id2, &rel2, container.as_str(), Some(itag), size)) {
            Ok(true) => {}
            Ok(false) => {
                // Already in the library: the extra copy is not needed.
                let _ = std::fs::remove_file(path);
                lock(&self.active).remove(id);
                return;
            }
            Err(e) => {
                tracing::warn!(%id, error = %e, "registering the cached stream failed");
                return;
            }
        }
        lock(&self.active).remove(id);
        if let Ok(l) = mixtape_audio::measure_loudness(path) {
            let id2 = id.clone();
            if self.db.write(move |t| repo::set_loudness(t, &id2, l.lufs, l.true_peak)).is_ok() {
                self.refresh_gain(id);
            }
        }
        let protect = lock(&self.wanted).clone();
        let limit = cache_lru::cache_limit(&self.db).unwrap_or(cache_lru::DEFAULT_CACHE_LIMIT);
        let root = self.root.clone();
        if let Ok(evicted) = self.db.write(move |t| cache_lru::evict(t, &root, limit, &protect))
            && !evicted.is_empty()
        {
            tracing::info!(n = evicted.len(), "stream cache trimmed");
        }
        crate::host::emit(&self.app, "queue:changed", ());
        if let Some(st) = self.app.try_state::<Arc<crate::state::Session>>()
            && let Some(e) = st.running()
        {
            e.analyzer.kick();
        }
    }

    /// Stops downloads nobody waits for any more and removes their partial files.
    fn drop_unwanted(&self, keep: &HashSet<&TrackId>) {
        let dropped: Vec<(TrackId, Slot)> = {
            let mut active = lock(&self.active);
            let ids: Vec<TrackId> = active.keys().filter(|id| !keep.contains(id)).cloned().collect();
            ids.into_iter().filter_map(|id| active.remove(&id).map(|s| (id, s))).collect()
        };
        for (id, slot) in dropped {
            if let Some(a) = slot.get()
                && !a.file.is_complete()
            {
                a.fill.cancel();
                a.file.cancel();
                let _ = std::fs::remove_file(self.part_path(&id, a.container));
            }
        }
    }

    /// Whether `id` has a URL or a download already.
    fn ready(&self, id: &TrackId) -> bool {
        lock(&self.active).contains_key(id) || self.resolver.cached(id).is_some()
    }
}

impl StreamProvider for AppStreams {
    fn open(&self, id: &TrackId, done: Opened) {
        let Some(s) = self.me.upgrade() else { return };
        let id = id.clone();
        let generation = self.generation.load(Ordering::SeqCst);
        // In parallel with the URL: a `player` call answers well before yt-dlp does.
        let (s2, id2) = (s.clone(), id.clone());
        self.rt.spawn(async move { s2.fetch_loudness(vec![id2]).await });
        self.rt.spawn(async move {
            if !s.wait_turn(&id, generation, false).await {
                done(Err(tr("The track is no longer in the queue", "Трека більше немає в черзі", "Трек больше не в очереди").into()));
                return;
            }
            match s.start(&id).await {
                Ok(a) => {
                    let _ = s.db.write({
                        let id = id.clone();
                        move |t| cache_lru::touch(t, &id)
                    });
                    done(Ok(Source::stream(id.as_str(), a.file.clone(), a.container.as_str(), 0.0)));
                }
                Err(message) => done(Err(message)),
            }
        });
    }

    fn prefetch(&self, current: Option<&TrackId>, upcoming: &[TrackId]) {
        *lock(&self.wanted) = current.into_iter().chain(upcoming).cloned().collect();
        *lock(&self.current) = current.cloned();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        // Partial downloads anywhere in the window survive a mere re-order.
        let keep: HashSet<&TrackId> = current.into_iter().chain(upcoming).collect();
        self.drop_unwanted(&keep);
        self.changed.notify_waiters();
        let Some(s) = self.me.upgrade() else { return };
        let cache: Vec<TrackId> = upcoming.iter().take(CACHE_AHEAD).cloned().collect();
        // Low-water mark: one yt-dlp run for the whole window once few URLs are left (or the
        // track to cache next lacks one), instead of a run per track change.
        let ready = upcoming.iter().filter(|id| self.ready(id)).count();
        let low = ready < RESOLVE_LOW_WATER.min(upcoming.len()) || cache.iter().any(|id| !self.ready(id));
        let to_resolve: Vec<TrackId> = match low {
            true => upcoming.iter().filter(|id| !self.ready(id)).cloned().collect(),
            false => Vec::new(),
        };
        let (s2, loudness) = (s.clone(), upcoming.to_vec());
        self.rt.spawn(async move { s2.fetch_loudness(loudness).await });
        self.rt.spawn(async move {
            if !to_resolve.is_empty() {
                let _ = s.resolver.resolve(&to_resolve).await;
            }
            for id in cache {
                if !s.wait_turn(&id, generation, true).await {
                    return;
                }
                if let Err(e) = s.start(&id).await {
                    tracing::debug!(%id, error = %e, "prefetch failed");
                }
            }
        });
    }
}
