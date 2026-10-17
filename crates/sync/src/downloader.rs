//! The download worker: takes queued jobs, resolves stream URLs through yt-dlp, fetches
//! the audio into `library/`, measures it and records it — at most two at a time, paced
//! so YouTube does not throttle the account.

use crate::disk;
use crate::fetch::{self, Cancel, FetchError};
use mixtape_core::TrackId;
use mixtape_library::repo::{self, FileLocation, MediaFile};
use mixtape_library::{Db, jobs};
use mixtape_yt::YtError;
use mixtape_yt::limits::{Outcome, Pacer};
use mixtape_yt::resolver::{ResolvedStream, StreamResolver};
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
/// Idle workers re-check the queue this often even without a wake-up.
const IDLE_POLL: Duration = Duration::from_secs(30);

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
        self.samples.retain(|(t, _)| now.duration_since(*t) < Duration::from_secs(2));
    }

    fn rate(&self) -> u64 {
        let Some((first, _)) = self.samples.first() else { return 0 };
        let span = first.elapsed().as_secs_f64().max(0.5);
        (self.samples.iter().map(|(_, b)| b).sum::<u64>() as f64 / span) as u64
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
}

impl Inner {
    fn pause(&self) {
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
        }
    }

    fn emit(&self, force: bool) {
        {
            let mut last = self.last_emit.lock().unwrap_or_else(|e| e.into_inner());
            if !force && last.elapsed() < EMIT_EVERY {
                return;
            }
            *last = Instant::now();
        }
        self.hooks.progress(&self.snapshot());
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
        if inner.paused.load(Ordering::SeqCst) {
            let _ = tokio::time::timeout(IDLE_POLL, inner.wake.notified()).await;
            continue;
        }
        let notified = inner.wake.notified();
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
            Ok(None) | Err(_) => {
                inner.emit(true);
                let _ = tokio::time::timeout(IDLE_POLL, notified).await;
                continue;
            }
        };
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
        inner.db.read(move |c| repo::track(c, &id)).ok().flatten().map(|t| t.title).unwrap_or_default()
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
    let mut stream = match inner.resolver.resolve_one(&id).await {
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
        fail(format!("не удалось сохранить файл: {e}")).await;
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
            repo::put_media_file(t, &media)?;
            if let (Some(lufs), Some(peak)) = (analysis.lufs, analysis.true_peak) {
                repo::set_loudness(t, &media.track_id, lufs, peak)?;
            }
            jobs::mark_done(t, job)
        })
        .await;
    match saved {
        Ok(_) => {
            inner.hooks.finished(&id);
            Outcome::Ok
        }
        Err(e) => {
            fail(format!("не удалось записать в библиотеку: {e}")).await;
            Outcome::OtherError
        }
    }
}

async fn fetch_with_progress(
    inner: &Arc<Inner>,
    job: i64,
    stream: &ResolvedStream,
    part: &Path,
    cancel: &Cancel,
) -> Result<u64, FetchError> {
    let mut last_db = Instant::now();
    let mut last_bytes = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let inner2 = inner.clone();
    fetch::fetch(&inner.http, &stream.url, part, stream.size, cancel, move |done, total| {
        inner2.speed.lock().unwrap_or_else(|e| e.into_inner()).add(done.saturating_sub(last_bytes));
        last_bytes = done;
        if let Some(a) = inner2.active.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&job) {
            a.bytes_done = done;
            a.bytes_total = total;
        }
        if last_db.elapsed() > Duration::from_secs(2) {
            last_db = Instant::now();
            let _ = inner2.db.write(move |t| jobs::progress(t, job, done, total));
        }
        inner2.emit(false);
    })
    .await
}
