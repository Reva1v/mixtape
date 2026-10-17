//! Streaming for tracks without audio on disk (US3): resolves the URL, fills a sparse
//! cache file from the network while the engine plays it, moves the finished file into
//! `cache/`, measures its loudness and keeps the cache under its limit. This is the
//! player's [`StreamProvider`].

use mixtape_audio::{SparseCacheFile, Source};
use mixtape_core::{AppError, TrackId};
use mixtape_library::{Db, cache_lru, repo};
use mixtape_player::prefetch::CACHE_AHEAD;
use mixtape_player::{Opened, StreamProvider};
use mixtape_sync::fetch;
use mixtape_sync::stream::StreamFill;
use mixtape_yt::resolver::{Container, StreamResolver};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::OnceCell;

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
    active: Mutex<HashMap<TrackId, Slot>>,
    /// The current track and the ones after it: kept from eviction and cancellation.
    wanted: Mutex<Vec<TrackId>>,
    /// Ids yt-dlp is resolving ahead right now.
    resolving: Mutex<HashSet<TrackId>>,
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
        http: mixtape_sync::reqwest::Client,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            app,
            rt,
            db,
            root,
            resolver,
            http,
            active: Mutex::new(HashMap::new()),
            wanted: Mutex::new(Vec::new()),
            resolving: Mutex::new(HashSet::new()),
        })
    }

    fn part_path(&self, id: &TrackId, c: Container) -> PathBuf {
        self.root.join("cache").join(format!("{}.{}.part", id.as_str(), c.as_str()))
    }

    fn rel_final(id: &TrackId, c: Container) -> String {
        format!("cache/{}.{}", id.as_str(), c.as_str())
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
            .map_err(|e| format!("Не удалось создать файл кэша: {e}"))?;
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

    /// googlevideo answered 403: resolve a fresh URL and point the download at it.
    fn refresh_url(&self, id: TrackId) {
        let Some(s) = self.me.upgrade() else { return };
        self.rt.spawn(async move {
            s.resolver.invalidate(&id);
            let Ok(stream) = s.resolver.resolve_one(&id).await else { return };
            let active = lock(&s.active).get(&id).and_then(|slot| slot.get().cloned());
            if let Some(a) = active {
                a.fill.retarget(stream.url);
            }
        });
    }

    /// Waits for the download to end; a complete file moves into `cache/` and gets its loudness.
    async fn finish(self: Arc<Self>, id: TrackId, a: Arc<Active>) {
        while !a.fill.done() && !a.file.is_complete() && !a.file.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if !a.file.is_complete() {
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
            let _ = self.db.write(move |t| repo::set_loudness(t, &id2, l.lufs, l.true_peak));
        }
        let protect = lock(&self.wanted).clone();
        let limit = cache_lru::cache_limit(&self.db).unwrap_or(cache_lru::DEFAULT_CACHE_LIMIT);
        let root = self.root.clone();
        if let Ok(evicted) = self.db.write(move |t| cache_lru::evict(t, &root, limit, &protect))
            && !evicted.is_empty()
        {
            tracing::info!(n = evicted.len(), "stream cache trimmed");
        }
        let _ = self.app.emit("queue:changed", ());
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
}

impl StreamProvider for AppStreams {
    fn open(&self, id: &TrackId, done: Opened) {
        let Some(s) = self.me.upgrade() else { return };
        let id = id.clone();
        self.rt.spawn(async move {
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
        let keep: HashSet<&TrackId> = current.into_iter().chain(upcoming.iter().take(CACHE_AHEAD)).collect();
        self.drop_unwanted(&keep);
        let Some(s) = self.me.upgrade() else { return };
        let to_resolve: Vec<TrackId> = {
            let mut resolving = lock(&self.resolving);
            upcoming
                .iter()
                .filter(|id| self.resolver.cached(id).is_none() && resolving.insert((*id).clone()))
                .cloned()
                .collect()
        };
        let cache: Vec<TrackId> = upcoming.iter().take(CACHE_AHEAD).cloned().collect();
        self.rt.spawn(async move {
            if !to_resolve.is_empty() {
                let _ = s.resolver.resolve(&to_resolve).await;
                let mut resolving = lock(&s.resolving);
                for id in &to_resolve {
                    resolving.remove(id);
                }
            }
            for id in cache {
                let still = lock(&s.wanted).contains(&id);
                if still && let Err(e) = s.start(&id).await {
                    tracing::debug!(%id, error = %e, "prefetch failed");
                }
            }
        });
    }
}
