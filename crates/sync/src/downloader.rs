//! The download worker: takes queued jobs, resolves stream URLs through yt-dlp, fetches
//! the audio into `library/`, measures it and records it — at most two at a time, paced
//! so YouTube does not throttle the account.

use crate::disk;
use crate::fetch::{self, Cancel, FetchError};
use mixtape_core::{TrackId, trf};
use mixtape_library::repo::{self, FileLocation, MediaFile};
use mixtape_library::{Db, jobs};
use mixtape_yt::YtError;
use mixtape_yt::limits::{Outcome, Pacer};
use mixtape_yt::resolver::{BATCH_SIZE, ResolvedStream, StreamResolver};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use ts_rs::TS;

const WORKERS: usize = 2;
/// Progress updates to the UI are capped at 4 per second.
const EMIT_EVERY: Duration = Duration::from_millis(250);
/// Idle workers re-check the queue this often even without a wake-up (a safety net: every
/// change to the queue wakes them).
const IDLE_POLL: Duration = Duration::from_secs(15 * 60);
/// A running job's byte count goes to the database at most this often.
const DB_PROGRESS_EVERY: Duration = Duration::from_secs(2);
/// Window of the download speed shown in the progress.
const SPEED_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DownloadsProgress {
    pub running: bool,
    pub paused: bool,
    pub done: u32,
    pub total: u32,
    pub failed: u32,
    #[ts(optional)]
    pub current: Option<CurrentDownload>,
    #[ts(type = "number")]
    pub bytes_per_sec: u64,
    /// Reading sources from YouTube Music before downloads start.
    #[ts(optional)]
    pub sync: Option<SyncStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyncStatus {
    /// What is being read, in the app language: "Reading likes", "Downloading covers"…
    pub label: String,
    pub done: u32,
    /// 0 when the amount is not known yet.
    pub total: u32,
    /// A SoundCloud / Bandcamp link being added, not the YouTube Music sync.
    pub import: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CurrentDownload {
    pub track_id: TrackId,
    pub title: String,
    #[ts(type = "number")]
    pub bytes_done: u64,
    #[ts(optional, type = "number")]
    pub bytes_total: Option<u64>,
}

/// Facts about a finished file, measured off the async runtime.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub sample_rate: Option<u32>,
    pub lufs: Option<f64>,
    pub true_peak: Option<f64>,
}

/// What the app plugs in: audio analysis and notifications to the UI.
pub trait Hooks: Send + Sync + 'static {
    /// Blocking; runs on a blocking thread.
    fn analyze(&self, path: &Path) -> Analysis;
    fn progress(&self, p: &DownloadsProgress);
    /// A track's audio landed in the library.
    fn finished(&self, track: &TrackId);
    /// Cookies were rejected; downloads are paused until the account is reconnected.
    fn auth_expired(&self);
    /// Less than `disk::MIN_FREE` left; downloads are paused.
    fn low_space(&self);
}

#[derive(Clone)]
pub struct Downloader {
    inner: Arc<Inner>,
}

struct Inner {
    rt: tokio::runtime::Handle,
    db: Db,
    resolver: Arc<StreamResolver>,
    http: reqwest::Client,
    pacer: Pacer,
    root: PathBuf,
    hooks: Arc<dyn Hooks>,
    paused: AtomicBool,
    wake: Notify,
    /// Running jobs by id: what to show and how to stop them.
    active: Mutex<HashMap<i64, Active>>,
    speed: Mutex<Speed>,
    last_emit: Mutex<Instant>,
    /// What the UI got last; an equal snapshot is not sent again.
    last_sent: Mutex<Option<DownloadsProgress>>,
    /// A send is scheduled and will read the latest state.
    emit_queued: AtomicBool,
    sync: Mutex<Option<SyncStatus>>,
}

struct Active {
    track_id: TrackId,
    title: String,
    bytes_done: u64,
    bytes_total: Option<u64>,
    cancel: Cancel,
}

/// Bytes per second over a sliding ~2 s window.
#[derive(Default)]
struct Speed {
    samples: Vec<(Instant, u64)>,
}

impl Speed {
    fn add(&mut self, bytes: u64) {
        let now = Instant::now();
        self.samples.push((now, bytes));
        self.samples.retain(|(t, _)| now.duration_since(*t) < SPEED_WINDOW);
    }

    /// 0 once nothing arrived for a while, so an idle snapshot stays the same.
    fn rate(&self) -> u64 {
        let now = Instant::now();
        let mut recent = self.samples.iter().filter(|(t, _)| now.duration_since(*t) < SPEED_WINDOW).peekable();
        let Some((first, _)) = recent.peek() else { return 0 };
        let span = now.duration_since(*first).as_secs_f64().max(0.5);
        (recent.map(|(_, b)| b).sum::<u64>() as f64 / span) as u64
    }
}

impl Downloader {
    /// Requeues jobs cut off by the last exit and starts the workers on `rt`.
    pub fn start(
        rt: &tokio::runtime::Handle,
        db: Db,
        resolver: Arc<StreamResolver>,
        http: reqwest::Client,
        root: PathBuf,
        hooks: Arc<dyn Hooks>,
    ) -> Self {
        let _ = db.write(|t| jobs::requeue_running(t));
        let paused = db.read(jobs::summary).map(|s| s.paused > 0 && s.queued == 0).unwrap_or(false);
        let inner = Arc::new(Inner {
            rt: rt.clone(),
            db,
            resolver,
            http,
            pacer: Pacer::new(WORKERS),
            root,
            hooks,
            paused: AtomicBool::new(paused),
            wake: Notify::new(),
            active: Mutex::new(HashMap::new()),
            speed: Mutex::new(Speed::default()),
            last_emit: Mutex::new(Instant::now() - EMIT_EVERY),
            last_sent: Mutex::new(None),
            emit_queued: AtomicBool::new(false),
            sync: Mutex::new(None),
        });
        for _ in 0..WORKERS {
            rt.spawn(worker(inner.clone()));
        }
        Self { inner }
    }

    /// New jobs were queued.
    pub fn wake(&self) {
        self.inner.wake.notify_waiters();
        self.inner.emit(true);
    }

    pub fn pause(&self) {
        self.inner.pause();
    }

    pub fn resume(&self) {
        self.inner.paused.store(false, Ordering::SeqCst);
        let _ = self.inner.db.write(|t| jobs::resume_all(t));
        self.wake();
    }

    /// Retries one failed job, or every failed job with `None`.
    pub fn retry(&self, id: Option<i64>) {
        let _ = self.inner.db.write(move |t| match id {
            Some(id) => jobs::retry(t, id).map(|_| ()),
            None => jobs::retry_all_failed(t).map(|_| ()),
        });
        if !self.inner.paused.load(Ordering::SeqCst) {
            self.wake();
        } else {
            self.inner.emit(true);
        }
    }

    pub fn snapshot(&self) -> DownloadsProgress {
        self.inner.snapshot()
    }

    /// Shows a source sync in the progress; `None` when it ends. A new label is sent at
    /// once, counter updates are throttled like download progress.
    pub fn set_sync(&self, status: Option<SyncStatus>) {
        let force = {
            let mut cur = self.inner.sync.lock().unwrap_or_else(|e| e.into_inner());
            let changed = cur.as_ref().map(|s| &s.label) != status.as_ref().map(|s| &s.label);
            *cur = status;
            changed
        };
        self.inner.emit(force);
    }
}

impl Inner {
    fn pause(self: &Arc<Self>) {
        self.paused.store(true, Ordering::SeqCst);
        let _ = self.db.write(|t| jobs::pause_all(t));
        for a in self.active.lock().unwrap_or_else(|e| e.into_inner()).values() {
            a.cancel.cancel();
        }
        self.emit(true);
    }

    fn snapshot(&self) -> DownloadsProgress {
        let s = self.db.read(jobs::summary).unwrap_or_default();
        let active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        let current = active.values().max_by_key(|a| a.bytes_done).map(|a| CurrentDownload {
            track_id: a.track_id.clone(),
            title: a.title.clone(),
            bytes_done: a.bytes_done,
            bytes_total: a.bytes_total,
        });
        DownloadsProgress {
            running: s.running > 0 || (s.queued > 0 && !self.paused.load(Ordering::SeqCst)),
            paused: self.paused.load(Ordering::SeqCst) && s.paused > 0,
            done: s.done,
            total: s.queued + s.running + s.paused + s.done + s.failed,
            failed: s.failed,
            current,
            bytes_per_sec: self.speed.lock().unwrap_or_else(|e| e.into_inner()).rate(),
            sync: self.sync.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        }
    }

    /// Sends the progress to the UI from a blocking thread (the snapshot reads the
    /// database). Calls while a send is pending fold into it; non-forced ones are throttled.
    fn emit(self: &Arc<Self>, force: bool) {
        {
            let mut last = self.last_emit.lock().unwrap_or_else(|e| e.into_inner());
            if !force && last.elapsed() < EMIT_EVERY {
                return;
            }
            *last = Instant::now();
        }
        if self.emit_queued.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        self.rt.spawn_blocking(move || {
            me.emit_queued.store(false, Ordering::SeqCst);
            me.send_progress();
        });
    }

    /// Sends the current snapshot unless the UI has exactly that one already. The lock is
    /// held across snapshot and send, so a later send never carries an older state.
    fn send_progress(&self) {
        let mut last = self.last_sent.lock().unwrap_or_else(|e| e.into_inner());
        let snap = self.snapshot();
        if last.as_ref() == Some(&snap) {
            return;
        }
        self.hooks.progress(&snap);
        *last = Some(snap);
    }

    async fn db_read<R: Send + 'static>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> mixtape_library::Result<R> + Send + 'static,
    ) -> mixtape_library::Result<R> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || db.read(f))
            .await
            .unwrap_or_else(|e| Err(mixtape_library::Error::Other(e.to_string())))
    }

    async fn db_write<R: Send + 'static>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> mixtape_library::Result<R> + Send + 'static,
    ) -> mixtape_library::Result<R> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || db.write(f))
            .await
            .unwrap_or_else(|e| Err(mixtape_library::Error::Other(e.to_string())))
    }
}

async fn worker(inner: Arc<Inner>) {
    loop {
        // Created before the checks: `notify_waiters` reaches it from here on.
        let notified = inner.wake.notified();
        if inner.paused.load(Ordering::SeqCst) {
            let _ = tokio::time::timeout(IDLE_POLL, notified).await;
            continue;
        }
        let job = inner
            .db_write(|t| {
                let job = jobs::next_queued(t)?;
                if let Some(j) = &job {
                    jobs::mark_running(t, j.id)?;
                }
                Ok(job)
            })
            .await;
        let job = match job {
            Ok(Some(job)) => job,
            // Nothing to do: whoever queues work wakes us (and emits), so no emit here.
            Ok(None) | Err(_) => {
                let _ = tokio::time::timeout(IDLE_POLL, notified).await;
                continue;
            }
        };
        // Already streamed into the cache: move it into the library instead of downloading.
        let (root, tid) = (inner.root.clone(), job.track_id.clone());
        if let Ok(true) = inner.db_write(move |t| mixtape_library::cache_lru::promote(t, &root, &tid)).await {
            let _ = inner.db_write(move |t| jobs::mark_done(t, job.id)).await;
            inner.hooks.finished(&job.track_id);
            inner.emit(true);
            continue;
        }
        let permit = inner.pacer.acquire().await;
        if inner.paused.load(Ordering::SeqCst) {
            // Paused while waiting for a slot: `pause_all` already parked the job.
            continue;
        }
        let outcome = download(&inner, job.id, job.track_id.clone()).await;
        inner.pacer.report(outcome);
        drop(permit);
        inner.active.lock().unwrap_or_else(|e| e.into_inner()).remove(&job.id);
        inner.emit(true);
    }
}

/// Runs one job to its end state and says how it went, for the pacer.
async fn download(inner: &Arc<Inner>, job: i64, id: TrackId) -> Outcome {
    if disk::low_space(&inner.root) {
        inner.hooks.low_space();
        inner.pause();
        return Outcome::OtherError;
    }
    let title = {
        let id = id.clone();
        inner.db_read(move |c| repo::track(c, &id)).await.ok().flatten().map(|t| t.title).unwrap_or_default()
    };
    let cancel = Cancel::new();
    inner.active.lock().unwrap_or_else(|e| e.into_inner()).insert(
        job,
        Active { track_id: id.clone(), title, bytes_done: 0, bytes_total: None, cancel: cancel.clone() },
    );
    inner.emit(true);

    let fail = |reason: String| async move {
        let _ = inner.db_write(move |t| jobs::mark_failed(t, job, &reason)).await;
    };
    let mut stream = match resolve_with_lookahead(inner, &id).await {
        Ok(s) => s,
        Err(YtError::RateLimited) => {
            let _ = inner.db_write(move |t| jobs::requeue(t, job)).await;
            return Outcome::RateLimited;
        }
        Err(YtError::Auth) => {
            inner.hooks.auth_expired();
            let _ = inner.db_write(move |t| jobs::requeue(t, job)).await;
            inner.pause();
            return Outcome::OtherError;
        }
        Err(e) => {
            fail(e.to_string()).await;
            return Outcome::OtherError;
        }
    };

    let rel = format!("library/{}.{}", id, stream.container.as_str());
    let path = inner.root.join(&rel);
    let part = fetch::part_path(&path);
    let mut refreshed = false;
    let size = loop {
        if let Err(e) = fetch::claim_part(&part, &fetch::part_key(stream.itag, &stream.url)) {
            fail(FetchError::Io(e).to_string()).await;
            return Outcome::OtherError;
        }
        match fetch_with_progress(inner, job, &stream, &part, &cancel).await {
            Ok(size) => break size,
            // Stream URLs expire; get a fresh one once and resume from the `.part`.
            Err(FetchError::Expired) if !refreshed => {
                refreshed = true;
                inner.resolver.invalidate(&id);
                match inner.resolver.resolve_one(&id).await {
                    Ok(s) => stream = s,
                    Err(e) => {
                        fail(e.to_string()).await;
                        return Outcome::OtherError;
                    }
                }
            }
            Err(FetchError::Cancelled) => return Outcome::Ok,
            Err(e) => {
                fail(e.to_string()).await;
                return Outcome::OtherError;
            }
        }
    };
    if let Err(e) = fetch::finalize(&part, &path) {
        fail(trf!("Couldn't save the file: {e}", "Не вдалося зберегти файл: {e}", "не удалось сохранить файл: {e}")).await;
        return Outcome::OtherError;
    }

    let hooks = inner.hooks.clone();
    let analysed = path.clone();
    let analysis = tokio::task::spawn_blocking(move || hooks.analyze(&analysed)).await.unwrap_or_default();
    let media = MediaFile {
        track_id: id.clone(),
        path: rel,
        container: stream.container.as_str().to_owned(),
        itag: Some(stream.itag),
        size_bytes: size,
        sample_rate: analysis.sample_rate,
        location: FileLocation::Library,
    };
    let saved = inner
        .db_write(move |t| {
            let replaced = store_download(t, &media)?;
            if let (Some(lufs), Some(peak)) = (analysis.lufs, analysis.true_peak) {
                repo::set_loudness(t, &media.track_id, lufs, peak)?;
            }
            jobs::mark_done(t, job)?;
            Ok(replaced)
        })
        .await;
    match saved {
        Ok(replaced) => {
            if let Some(old) = replaced
                && let Err(e) = std::fs::remove_file(inner.root.join(&old))
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(%id, error = %e, "replaced audio file left behind");
            }
            inner.hooks.finished(&id);
            Outcome::Ok
        }
        Err(e) => {
            fail(trf!(
                "Couldn't write to the library: {e}",
                "Не вдалося записати в бібліотеку: {e}",
                "не удалось записать в библиотеку: {e}",
            ))
            .await;
            Outcome::OtherError
        }
    }
}

/// Records a finished download. Returns the path of the file it replaces (a cached copy
/// streamed meanwhile, or an older download in another container), for the caller to
/// delete once the row is committed.
fn store_download(t: &rusqlite::Connection, media: &MediaFile) -> mixtape_library::Result<Option<String>> {
    let old = repo::media_file(t, &media.track_id)?.map(|o| o.path).filter(|p| *p != media.path);
    repo::put_media_file(t, media)?;
    Ok(old)
}

/// The URL for `id`. On a cache miss the next queued tracks are resolved in the same
/// yt-dlp run (one process per [`BATCH_SIZE`] tracks instead of one per track), so their
/// jobs find the URL cached. Runs inside the caller's pacer slot.
async fn resolve_with_lookahead(inner: &Arc<Inner>, id: &TrackId) -> Result<ResolvedStream, YtError> {
    if let Some(s) = inner.resolver.cached(id) {
        return Ok(s);
    }
    let ahead = inner.db_read(|c| jobs::next_queued_tracks(c, BATCH_SIZE)).await.unwrap_or_default();
    let mut ids = vec![id.clone()];
    ids.extend(ahead.into_iter().filter(|t| t != id && inner.resolver.cached(t).is_none()).take(BATCH_SIZE - 1));
    inner
        .resolver
        .resolve(&ids)
        .await
        .into_iter()
        .next()
        .map(|(_, r)| r)
        .unwrap_or_else(|| Err(mixtape_yt::resolver::no_data()))
}

async fn fetch_with_progress(
    inner: &Arc<Inner>,
    job: i64,
    stream: &ResolvedStream,
    part: &Path,
    cancel: &Cancel,
) -> Result<u64, FetchError> {
    let mut last_db = Instant::now();
    let db_busy = Arc::new(());
    let mut last_bytes = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let inner2 = inner.clone();
    fetch::fetch(&inner.http, &stream.url, part, stream.size, cancel, move |done, total| {
        inner2.speed.lock().unwrap_or_else(|e| e.into_inner()).add(done.saturating_sub(last_bytes));
        last_bytes = done;
        if let Some(a) = inner2.active.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&job) {
            a.bytes_done = done;
            a.bytes_total = total;
        }
        // Off the async runtime, and never more than one write in flight.
        if last_db.elapsed() > DB_PROGRESS_EVERY && Arc::strong_count(&db_busy) == 1 {
            last_db = Instant::now();
            let (db, busy) = (inner2.db.clone(), db_busy.clone());
            inner2.rt.spawn_blocking(move || {
                let _ = db.write(move |t| jobs::progress(t, job, done, total));
                drop(busy);
            });
        }
        inner2.emit(false);
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<DownloadsProgress>>);

    impl Hooks for Recorder {
        fn analyze(&self, _: &Path) -> Analysis {
            Analysis::default()
        }
        fn progress(&self, p: &DownloadsProgress) {
            self.0.lock().unwrap().push(p.clone());
        }
        fn finished(&self, _: &TrackId) {}
        fn auth_expired(&self) {}
        fn low_space(&self) {}
    }

    impl Recorder {
        fn count(&self) -> usize {
            self.0.lock().unwrap().len()
        }

        fn sent(&self, i: usize) -> DownloadsProgress {
            self.0.lock().unwrap()[i].clone()
        }

        /// Waits until `n` sends arrived (or a while), plus room for a stray extra one.
        async fn settle(&self, n: usize) {
            let t = Instant::now();
            while self.count() < n && t.elapsed() < Duration::from_secs(5) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    #[test]
    fn a_download_reports_the_cached_copy_it_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let id = TrackId::new("aaaaaaaaaaa");
        let file = |path: &str, location| MediaFile {
            track_id: id.clone(),
            path: path.into(),
            container: "webm".into(),
            itag: Some(251),
            size_bytes: 1,
            sample_rate: None,
            location,
        };
        let (cached, again) = (file("cache/aaaaaaaaaaa.webm", FileLocation::Cache), file("library/aaaaaaaaaaa.webm", FileLocation::Library));
        let lib = again.clone();
        let (first, second) = db
            .write(move |t| {
                t.execute("INSERT INTO tracks(id, title, duration_ms, state) VALUES ('aaaaaaaaaaa', 'A', 1000, 'remote')", [])?;
                repo::put_media_file(t, &cached)?;
                Ok((store_download(t, &lib)?, store_download(t, &again)?))
            })
            .unwrap();
        assert_eq!(first.as_deref(), Some("cache/aaaaaaaaaaa.webm"));
        assert_eq!(second, None, "the same file is not deleted");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_and_repeated_progress_is_not_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let resolver = Arc::new(StreamResolver::new(
            "Z:/nope/yt-dlp.exe".into(),
            "deno.exe".into(),
            dir.path().join("tmp"),
        ));
        let hooks = Arc::new(Recorder::default());
        let dl = Downloader::start(
            &tokio::runtime::Handle::current(),
            db,
            resolver,
            reqwest::Client::new(),
            dir.path().to_path_buf(),
            hooks.clone(),
        );
        hooks.settle(1).await;
        assert_eq!(hooks.count(), 0, "idle workers on an empty queue send nothing");

        dl.wake();
        hooks.settle(1).await;
        assert_eq!(hooks.count(), 1);

        // Nothing changed: nothing new is sent.
        dl.wake();
        dl.wake();
        dl.set_sync(None);
        dl.retry(None);
        hooks.settle(2).await;
        assert_eq!(hooks.count(), 1);

        let status = SyncStatus { label: "Читаю лайки".into(), done: 0, total: 0, import: false };
        dl.set_sync(Some(status.clone()));
        hooks.settle(2).await;
        assert_eq!(hooks.count(), 2);
        assert_eq!(hooks.sent(1).sync, Some(status));
        dl.set_sync(None);
        hooks.settle(3).await;
        assert_eq!(hooks.count(), 3);
        assert_eq!(hooks.sent(2), hooks.sent(0));
    }
}
