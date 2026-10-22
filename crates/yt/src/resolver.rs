//! Direct audio URLs through the yt-dlp sidecar.
//!
//! yt-dlp runs once per batch of up to five videos with `-f 251/140/… -J` (see `FORMATS`). It prints one
//! JSON object per video (`null` for a failed one) and `ERROR: [youtube] <id>: …` lines
//! on stderr, which become reasons in the app language. URLs live about six hours and are cached in
//! memory. yt-dlp rewrites its cookie file on exit, so every call gets a fresh temporary
//! copy of the cookies that is deleted right after the call. An id that is being resolved
//! already is not passed to a second yt-dlp: later callers wait for the first one's answer.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mixtape_core::{TrackId, tr, trf};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;

use crate::{Result, YtError};

/// Videos per yt-dlp process.
pub const BATCH_SIZE: usize = 5;
/// Wall-clock limit for one yt-dlp process.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// A cached URL is not handed out when it expires sooner than this.
const EXPIRY_MARGIN_SECS: u64 = 10 * 60;
/// Lifetime assumed when the URL carries no `expire=`.
const FALLBACK_TTL_SECS: u64 = 5 * 60 * 60;
const COOKIE_PREFIX: &str = "ytdlp-cookies-";
const NETSCAPE_HEADER: &str = "# Netscape HTTP Cookie File";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    /// itag 251, Opus in WebM.
    Webm,
    /// itag 140, AAC in MP4.
    M4a,
}

impl Container {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webm => "webm",
            Self::M4a => "m4a",
        }
    }

    fn detect(ext: Option<&str>, itag: u32) -> Option<Self> {
        match (ext, itag) {
            (Some("webm"), _) => Some(Self::Webm),
            (Some("m4a" | "mp4"), _) => Some(Self::M4a),
            (_, 251) => Some(Self::Webm),
            (_, 140) => Some(Self::M4a),
            _ => None,
        }
    }
}

/// Opus 160k, then AAC 128k, then the smaller Opus/AAC streams and any audio in a
/// container the library accepts — some videos lack 251/140.
const FORMATS: &str = "251/140/250/249/139/bestaudio[ext=webm]/bestaudio[ext=m4a]";

/// A direct, time-limited audio URL for one video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStream {
    pub url: String,
    pub itag: u32,
    pub container: Container,
    /// Exact `filesize`, else yt-dlp's `filesize_approx`.
    pub size: Option<u64>,
    pub duration_ms: u64,
    /// Unix seconds after which googlevideo rejects the URL.
    pub expires_at: u64,
}

impl ResolvedStream {
    /// Whether the URL is still good enough to start a download at `now` (unix seconds).
    pub fn is_fresh(&self, now: u64) -> bool {
        now + EXPIRY_MARGIN_SECS < self.expires_at
    }
}

/// The answer of [`Resolve::resolve_batch`]: each id with its result, in order.
pub type BatchFuture<'a> = Pin<Box<dyn Future<Output = Vec<(TrackId, Result<ResolvedStream>)>> + Send + 'a>>;

/// The answer of [`Resolve::resolve_one`].
pub type ResolveFuture<'a> = Pin<Box<dyn Future<Output = Result<ResolvedStream>> + Send + 'a>>;

/// Something that turns a video id into a direct audio URL: yt-dlp on the desktop,
/// [`crate::direct::DirectResolver`] on Android.
pub trait Resolve: Send + Sync {
    fn resolve_one<'a>(&'a self, id: &'a TrackId) -> ResolveFuture<'a>;
    /// Forgets the cached URL, e.g. after googlevideo answered 403.
    fn invalidate(&self, id: &TrackId);
    /// A URL still in memory, without asking YouTube.
    fn cached(&self, _id: &TrackId) -> Option<ResolvedStream> {
        None
    }
    /// Several ids in one go where the resolver can batch them (one yt-dlp run); by default
    /// only the first id is resolved and the rest are left for later calls.
    fn resolve_batch<'a>(&'a self, ids: &'a [TrackId]) -> BatchFuture<'a> {
        Box::pin(async move {
            match ids.first() {
                Some(id) => vec![(id.clone(), self.resolve_one(id).await)],
                None => Vec::new(),
            }
        })
    }
}

impl Resolve for StreamResolver {
    fn resolve_one<'a>(&'a self, id: &'a TrackId) -> ResolveFuture<'a> {
        Box::pin(StreamResolver::resolve_one(self, id))
    }

    fn invalidate(&self, id: &TrackId) {
        StreamResolver::invalidate(self, id);
    }

    fn cached(&self, id: &TrackId) -> Option<ResolvedStream> {
        StreamResolver::cached(self, id)
    }

    fn resolve_batch<'a>(&'a self, ids: &'a [TrackId]) -> BatchFuture<'a> {
        Box::pin(StreamResolver::resolve(self, ids))
    }
}

/// Resolves video ids to audio URLs with yt-dlp, caching the results in memory.
pub struct StreamResolver {
    ytdlp: PathBuf,
    deno: PathBuf,
    tmp_dir: PathBuf,
    timeout: Duration,
    cookies: Mutex<Option<String>>,
    cache: Mutex<HashMap<TrackId, ResolvedStream>>,
    /// Ids a yt-dlp process is resolving right now; the answer arrives through the channel.
    inflight: Mutex<HashMap<TrackId, Pending>>,
    seq: AtomicU64,
}

/// The answer for one id in flight: `None` until its yt-dlp process ends.
type Pending = watch::Receiver<Option<Result<ResolvedStream>>>;

impl fmt::Debug for StreamResolver {
    // Hand-written so cookie values never end up in logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamResolver")
            .field("ytdlp", &self.ytdlp)
            .field("deno", &self.deno)
            .field("tmp_dir", &self.tmp_dir)
            .field("has_cookies", &lock(&self.cookies).is_some())
            .finish_non_exhaustive()
    }
}

impl StreamResolver {
    /// `tmp_dir` receives the short-lived cookie copies; leftovers from a crashed run are
    /// removed here.
    pub fn new(ytdlp: PathBuf, deno: PathBuf, tmp_dir: PathBuf) -> Self {
        purge_cookie_copies(&tmp_dir);
        Self {
            ytdlp,
            deno,
            tmp_dir,
            timeout: CALL_TIMEOUT,
            cookies: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(0),
        }
    }

    /// Overrides the per-process limit (tests).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The yt-dlp executable (also used for SoundCloud / Bandcamp imports).
    pub fn ytdlp(&self) -> &Path {
        &self.ytdlp
    }

    /// Netscape `cookies.txt` text passed to yt-dlp, or `None` to resolve anonymously.
    pub fn set_cookies(&self, netscape: Option<String>) {
        *lock(&self.cookies) = netscape;
    }

    /// Resolves every id, at most [`BATCH_SIZE`] per yt-dlp process, cache first. Ids
    /// another call is resolving already are awaited instead of resolved twice.
    /// The result has one entry per input id, in input order.
    pub async fn resolve(&self, ids: &[TrackId]) -> Vec<(TrackId, Result<ResolvedStream>)> {
        let mut done: HashMap<TrackId, Result<ResolvedStream>> = HashMap::new();
        let mut todo: Vec<TrackId> = Vec::new();
        for id in ids {
            if !todo.contains(id) {
                todo.push(id.clone());
            }
        }
        while !todo.is_empty() {
            let now = now_unix();
            let mut claim = Claim {
                resolver: self,
                ids: Vec::new(),
            };
            let mut waits: Vec<(TrackId, Pending)> = Vec::new();
            {
                let mut inflight = lock(&self.inflight);
                for id in todo.drain(..) {
                    if let Some(s) = self.cache_get(&id, now) {
                        done.insert(id, Ok(s));
                    } else if let Some(rx) = inflight.get(&id) {
                        waits.push((id, rx.clone()));
                    } else {
                        let (tx, rx) = watch::channel(None);
                        inflight.insert(id.clone(), rx);
                        claim.ids.push((id, tx));
                    }
                }
            }
            let mine: Vec<TrackId> = claim.ids.iter().map(|(id, _)| id.clone()).collect();
            for chunk in mine.chunks(BATCH_SIZE) {
                for (id, res) in self.run_batch(chunk).await {
                    if let Ok(s) = &res {
                        self.cache_put(id.clone(), s.clone());
                    }
                    claim.publish(&id, &res);
                    done.insert(id, res);
                }
            }
            drop(claim);
            for (id, mut rx) in waits {
                let answer = rx
                    .wait_for(Option::is_some)
                    .await
                    .ok()
                    .and_then(|v| v.as_ref().map(clone_result));
                match answer {
                    Some(res) => {
                        done.insert(id, res);
                    }
                    // The call resolving it was dropped before the answer: try again.
                    None => todo.push(id),
                }
            }
        }
        ids.iter()
            .map(|id| {
                let res = match done.get(id) {
                    Some(r) => clone_result(r),
                    None => Err(no_data()),
                };
                (id.clone(), res)
            })
            .collect()
    }

    pub async fn resolve_one(&self, id: &TrackId) -> Result<ResolvedStream> {
        self.resolve(std::slice::from_ref(id))
            .await
            .pop()
            .map(|(_, r)| r)
            .unwrap_or_else(|| Err(no_data()))
    }

    /// A cached URL that is still fresh, without running yt-dlp.
    pub fn cached(&self, id: &TrackId) -> Option<ResolvedStream> {
        self.cache_get(id, now_unix())
    }

    /// Forgets the cached URL, e.g. after googlevideo answered 403.
    pub fn invalidate(&self, id: &TrackId) {
        lock(&self.cache).remove(id);
    }

    pub fn clear_cache(&self) {
        lock(&self.cache).clear();
    }

    fn cache_get(&self, id: &TrackId, now: u64) -> Option<ResolvedStream> {
        let mut cache = lock(&self.cache);
        match cache.get(id) {
            Some(s) if s.is_fresh(now) => Some(s.clone()),
            Some(_) => {
                cache.remove(id);
                None
            }
            None => None,
        }
    }

    /// Stores a URL and drops the entries that went stale meanwhile.
    fn cache_put(&self, id: TrackId, stream: ResolvedStream) {
        let now = now_unix();
        let mut cache = lock(&self.cache);
        cache.retain(|_, s| s.is_fresh(now));
        cache.insert(id, stream);
    }

    async fn run_batch(&self, ids: &[TrackId]) -> Vec<(TrackId, Result<ResolvedStream>)> {
        // Ids go into yt-dlp's arguments; one that is not a plain id never gets there.
        if ids.iter().any(|id| !id.is_safe()) {
            let (safe, bad): (Vec<TrackId>, Vec<TrackId>) =
                ids.iter().cloned().partition(TrackId::is_safe);
            let mut out: Vec<(TrackId, Result<ResolvedStream>)> = bad
                .into_iter()
                .map(|id| {
                    let reason = trf!("invalid id {id}", "неправильний id {id}", "неверный id {id}");
                    (id, Err(YtError::Unavailable(reason)))
                })
                .collect();
            if !safe.is_empty() {
                out.extend(Box::pin(self.run_batch(&safe)).await);
            }
            return out;
        }
        let started = Instant::now();
        let cookies = match self.write_cookie_copy().await {
            Ok(c) => c,
            Err(e) => {
                let reason = trf!(
                    "couldn't prepare cookies: {e}",
                    "не вдалося підготувати cookies: {e}",
                    "не удалось подготовить cookies для yt-dlp: {e}",
                );
                return ids
                    .iter()
                    .map(|id| (id.clone(), Err(YtError::Tool(reason.clone()))))
                    .collect();
            }
        };
        let out = self
            .run_ytdlp(ids, cookies.as_ref().map(CookieCopy::path))
            .await;
        drop(cookies);
        let results = attribute(ids, &out, now_unix());
        let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
        tracing::debug!(
            batch = ids.len(),
            ok,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "yt-dlp batch resolved"
        );
        if let Some(f) = &out.failure {
            tracing::warn!("yt-dlp: {f}");
        }
        results
    }

    async fn write_cookie_copy(&self) -> std::io::Result<Option<CookieCopy>> {
        let Some(text) = lock(&self.cookies).clone() else {
            return Ok(None);
        };
        tokio::fs::create_dir_all(&self.tmp_dir).await?;
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            "{COOKIE_PREFIX}{}-{n}-{:08x}.txt",
            std::process::id(),
            fastrand::u32(..)
        );
        // The guard exists before the write, so even a partial file is removed.
        let copy = CookieCopy(self.tmp_dir.join(name));
        let body = if text.trim_start().starts_with('#') {
            text
        } else {
            format!("{NETSCAPE_HEADER}\n{text}")
        };
        tokio::fs::write(copy.path(), body).await?;
        Ok(Some(copy))
    }

    async fn run_ytdlp(&self, ids: &[TrackId], cookies: Option<&Path>) -> RunOutput {
        if let Err(e) = tokio::fs::create_dir_all(&self.tmp_dir).await {
            return RunOutput::failed(trf!(
                "couldn't create the temporary folder: {e}",
                "не вдалося створити тимчасову папку: {e}",
                "не удалось создать временную папку: {e}",
            ));
        }
        let mut cmd = ytdlp_command(&self.ytdlp, &self.tmp_dir);
        if let Some(path) = cookies {
            cmd.arg("--cookies").arg(path);
        }
        let mut runtime = OsString::from("deno:");
        runtime.push(&self.deno);
        cmd.arg("--js-runtimes")
            .arg(runtime)
            .args([
                "-f",
                FORMATS,
                "-J",
                "--no-warnings",
                "--no-playlist",
                "--ignore-errors",
            ])
            .args(["--encoding", "utf-8", "--"])
            .args(
                ids.iter()
                    .map(|id| format!("https://www.youtube.com/watch?v={id}")),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return RunOutput::failed(trf!(
                    "couldn't start: {e}",
                    "не вдалося запустити: {e}",
                    "не удалось запустить yt-dlp: {e}",
                ));
            }
        };
        let pid = child.id();
        let mut guard = TreeGuard(pid);
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let finished = tokio::time::timeout(self.timeout, async {
            let (_, _, status) = tokio::join!(
                read_all(&mut stdout, &mut out),
                read_all(&mut stderr, &mut err),
                child.wait()
            );
            status
        })
        .await;
        guard.disarm();
        let failure = match finished {
            Ok(Ok(_)) => None,
            Ok(Err(e)) => Some(trf!("wait failed: {e}", "помилка очікування: {e}", "ошибка ожидания yt-dlp: {e}")),
            Err(_) => {
                kill_tree(pid, &mut child).await;
                let secs = self.timeout.as_secs();
                Some(trf!(
                    "no answer in {secs} s",
                    "немає відповіді за {secs} с",
                    "yt-dlp не ответил за {secs} с",
                ))
            }
        };
        RunOutput {
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            failure,
        }
    }
}

/// Ids one `resolve` call took on. Whatever it did not answer leaves the in-flight map when
/// the call ends or is dropped, so the waiters retry instead of hanging.
struct Claim<'a> {
    resolver: &'a StreamResolver,
    ids: Vec<(TrackId, watch::Sender<Option<Result<ResolvedStream>>>)>,
}

impl Claim<'_> {
    /// Hands the answer for `id` to everyone waiting for it.
    fn publish(&mut self, id: &TrackId, res: &Result<ResolvedStream>) {
        let Some(i) = self.ids.iter().position(|(c, _)| c == id) else {
            return;
        };
        let (id, tx) = self.ids.swap_remove(i);
        lock(&self.resolver.inflight).remove(&id);
        tx.send_replace(Some(clone_result(res)));
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let mut inflight = lock(&self.resolver.inflight);
        for (id, _) in &self.ids {
            inflight.remove(id);
        }
    }
}

/// What one yt-dlp process left behind.
#[derive(Debug, Default)]
struct RunOutput {
    stdout: String,
    stderr: String,
    /// The process did not start or did not finish in time.
    failure: Option<String>,
}

impl RunOutput {
    fn failed(reason: String) -> Self {
        Self {
            failure: Some(reason),
            ..Self::default()
        }
    }
}

/// Temporary cookie file, removed when dropped.
struct CookieCopy(PathBuf);

impl CookieCopy {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for CookieCopy {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("не удалось удалить временную копию cookies: {e}");
        }
    }
}

fn purge_cookie_copies(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(COOKIE_PREFIX)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

async fn read_all<R: AsyncRead + Unpin>(src: &mut Option<R>, buf: &mut Vec<u8>) {
    if let Some(r) = src {
        let _ = r.read_to_end(buf).await;
    }
}

/// A yt-dlp command hardened against the user's environment: no config files
/// (a `yt-dlp.conf` next to the exe, in `%APPDATA%` or in the working directory could add
/// `--exec` or change output paths) and no plugins, run in `work_dir` (the app's own
/// temporary folder, which must exist) instead of whatever directory the app started in.
/// Callers put `--` before URLs and ids.
pub fn ytdlp_command(exe: &Path, work_dir: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(["--ignore-config", "--no-plugin-dirs"])
        .current_dir(work_dir);
    cmd
}

/// `%SystemRoot%\System32\<name>`, so a same-named program in the working directory or on
/// `PATH` never runs instead.
#[cfg(windows)]
pub fn system32(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join(name)
}

/// Kills the child's whole process tree if dropped while armed: when the future that runs
/// yt-dlp is cancelled, `kill_on_drop` alone would stop only the PyInstaller launcher and
/// leave Python (and deno) running.
struct TreeGuard(Option<u32>);

impl TreeGuard {
    /// The process has exited or is handled elsewhere: its PID may be reused, never kill by it.
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TreeGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(pid) = self.0.take() {
            use std::os::windows::process::CommandExt as _;
            // Waits (taskkill takes tens of milliseconds): the tree must be walked before
            // `kill_on_drop` takes the launcher down and orphans its children. Only cancelled
            // calls get here.
            let _ = std::process::Command::new(system32("taskkill.exe"))
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }
    }
}

/// yt-dlp.exe is a PyInstaller bundle that runs Python (and deno) as child processes,
/// so the whole tree is killed by PID, not just the launcher.
async fn kill_tree(pid: Option<u32>, child: &mut Child) {
    #[cfg(windows)]
    if let Some(pid) = pid {
        let mut taskkill = Command::new(system32("taskkill.exe"));
        taskkill
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let _ = tokio::time::timeout(Duration::from_secs(5), taskkill.status()).await;
    }
    #[cfg(not(windows))]
    let _ = pid;
    let _ = child.kill().await;
}

/// Runs a prepared sidecar command (see [`ytdlp_command`]) without a console window and
/// collects its output. After `timeout`, or when the returned future is dropped, the whole
/// process tree is killed. `Err` carries a reason in the app
/// language when the process did not start or did not finish; the exit code is not checked.
pub async fn run_capture(mut cmd: Command, timeout: Duration) -> std::result::Result<(String, String), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd
        .spawn()
        .map_err(|e| trf!("couldn't start yt-dlp: {e}", "не вдалося запустити yt-dlp: {e}", "не удалось запустить yt-dlp: {e}"))?;
    let pid = child.id();
    let mut guard = TreeGuard(pid);
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let finished = tokio::time::timeout(timeout, async {
        let (_, _, status) = tokio::join!(read_all(&mut stdout, &mut out), read_all(&mut stderr, &mut err), child.wait());
        status
    })
    .await;
    // From here on the timeout branch kills the tree itself, and an exited process's PID
    // could soon belong to someone else.
    guard.disarm();
    match finished {
        Ok(Ok(_)) => Ok((String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned())),
        Ok(Err(e)) => Err(trf!("yt-dlp wait failed: {e}", "помилка очікування yt-dlp: {e}", "ошибка ожидания yt-dlp: {e}")),
        Err(_) => {
            kill_tree(pid, &mut child).await;
            let secs = timeout.as_secs();
            Err(trf!("yt-dlp didn't answer in {secs} s", "yt-dlp не відповів за {secs} с", "yt-dlp не ответил за {secs} с"))
        }
    }
}

/// Matches yt-dlp's JSON lines and error lines to the requested ids.
fn attribute(ids: &[TrackId], out: &RunOutput, now: u64) -> Vec<(TrackId, Result<ResolvedStream>)> {
    let mut streams: HashMap<String, Result<ResolvedStream>> = HashMap::new();
    for line in out.stdout.lines().map(str::trim) {
        if line.is_empty() || line == "null" {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => {
                if let Some(id) = v.get("id").and_then(Value::as_str) {
                    streams.insert(id.to_owned(), parse_info(&v, now));
                }
            }
            Err(e) => tracing::warn!("yt-dlp вывел не JSON: {e}"),
        }
    }

    let mut by_id: HashMap<String, String> = HashMap::new();
    let mut general: Option<String> = None;
    for line in out.stderr.lines() {
        match parse_error_line(line) {
            Some((Some(id), msg)) => {
                by_id.entry(id.to_owned()).or_insert_with(|| msg.to_owned());
            }
            Some((None, msg)) => general = Some(msg.to_owned()),
            None => {}
        }
    }
    if general.is_none() {
        // A crash prints a traceback instead of an ERROR line; its last line says the most.
        general = out
            .stderr
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty() && !l.starts_with("ERROR:"))
            .map(str::to_owned);
    }

    ids.iter()
        .map(|id| {
            let res = if let Some(r) = streams.remove(id.as_str()) {
                r
            } else if let Some(msg) = by_id.get(id.as_str()) {
                Err(classify(msg))
            } else if let Some(f) = &out.failure {
                Err(YtError::Tool(f.clone()))
            } else if let Some(msg) = &general {
                Err(classify(msg))
            } else {
                Err(no_data())
            };
            (id.clone(), res)
        })
        .collect()
}

/// yt-dlp printed nothing about a video.
pub fn no_data() -> YtError {
    YtError::Tool(
        tr(
            "returned no video data",
            "не повернув даних про відео",
            "не вернул данные о видео",
        )
        .into(),
    )
}

/// Builds a [`ResolvedStream`] from one `-J` object.
fn parse_info(v: &Value, now: u64) -> Result<ResolvedStream> {
    let f = selected_format(v)
        .ok_or_else(|| YtError::Parse(
            tr(
                "yt-dlp picked no format",
                "у відповіді yt-dlp немає вибраного формату",
                "в ответе yt-dlp нет выбранного формата",
            )
            .into(),
        ))?;
    let url = f
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| YtError::Parse(
            tr(
                "yt-dlp gave no stream link",
                "у відповіді yt-dlp немає посилання на потік",
                "в ответе yt-dlp нет ссылки на поток",
            )
            .into(),
        ))?
        .to_owned();
    let format_id = f
        .get("format_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let digits: String = format_id.chars().take_while(char::is_ascii_digit).collect();
    let itag: u32 = digits
        .parse()
        .map_err(|_| YtError::Parse(trf!(
            "unknown yt-dlp format: {format_id:?}",
            "незрозумілий формат yt-dlp: {format_id:?}",
            "непонятный формат yt-dlp: {format_id:?}",
        )))?;
    let container = Container::detect(f.get("ext").and_then(Value::as_str), itag)
        .ok_or_else(|| YtError::Parse(trf!(
            "unexpected format {format_id}",
            "неочікуваний формат {format_id}",
            "неожиданный формат {format_id}",
        )))?;
    let size = as_u64(f.get("filesize")).or_else(|| as_u64(f.get("filesize_approx")));
    let duration_ms = v
        .get("duration")
        .and_then(Value::as_f64)
        .map(|d| (d * 1000.0).round() as u64)
        .unwrap_or(0);
    let expires_at = parse_expire(&url).unwrap_or(now + FALLBACK_TTL_SECS);
    Ok(ResolvedStream {
        url,
        itag,
        container,
        size,
        duration_ms,
        expires_at,
    })
}

/// The format yt-dlp picked: the top level for a single format, else the first
/// downloadable entry of `requested_downloads` / `requested_formats` (audio first).
fn selected_format(v: &Value) -> Option<&Value> {
    let has_url = |f: &Value| f.get("url").and_then(Value::as_str).is_some();
    if has_url(v) && v.get("format_id").is_some() {
        return Some(v);
    }
    for key in ["requested_downloads", "requested_formats"] {
        let Some(list) = v.get(key).and_then(Value::as_array) else {
            continue;
        };
        let audio = list
            .iter()
            .find(|f| has_url(f) && f.get("vcodec").and_then(Value::as_str) == Some("none"));
        if let Some(f) = audio.or_else(|| list.iter().find(|f| has_url(f))) {
            return Some(f);
        }
    }
    None
}

fn as_u64(v: Option<&Value>) -> Option<u64> {
    let v = v?;
    v.as_u64()
        .or_else(|| v.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
}

/// Unix seconds from googlevideo's `expire=` query parameter (or the older `/expire/<n>/`).
pub fn parse_expire(url: &str) -> Option<u64> {
    if let Some((_, query)) = url.split_once('?') {
        let found = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("expire="))
            .and_then(|n| n.parse().ok());
        if found.is_some() {
            return found;
        }
    }
    let rest = &url[url.find("/expire/")? + "/expire/".len()..];
    rest.split('/').next()?.parse().ok()
}

/// Splits `ERROR: [youtube] <id>: <message>` into the video id (when present) and message.
fn parse_error_line(line: &str) -> Option<(Option<&str>, &str)> {
    let rest = line.trim().strip_prefix("ERROR:")?.trim_start();
    if let Some(after) = rest.strip_prefix('[')
        && let Some(end) = after.find(']')
    {
        let tail = after[end + 1..].trim_start();
        if let Some((id, msg)) = tail.split_once(": ")
            && is_video_id(id)
        {
            return Some((Some(id), msg.trim()));
        }
        return Some((None, tail));
    }
    Some((None, rest))
}

fn is_video_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Turns a yt-dlp error message into an error with a reason people understand.
pub fn classify(message: &str) -> YtError {
    let m = message.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    let unavailable = |s: &str| YtError::Unavailable(s.to_owned());

    if has(&[
        "not a bot",
        "http error 429",
        "too many requests",
        "rate-limited",
        "try again later",
    ]) {
        YtError::RateLimited
    } else if has(&["copyright", "umg", "content id", "who has blocked it"]) {
        unavailable(tr("Blocked by the copyright holder", "Заблоковано правовласником", "Заблокировано правообладателем"))
    } else if has(&["in your country", "geo restricted", "geo-restricted"]) {
        unavailable(tr("Not available in your country", "Недоступно у вашій країні", "Недоступно в вашей стране"))
    } else if has(&["requested format is not available"]) {
        unavailable(tr("No suitable audio track", "Немає відповідної аудіодоріжки", "Нет подходящей аудиодорожки"))
    } else if has(&["private video"]) {
        unavailable(tr("Private video", "Приватне відео", "Приватное видео"))
    } else if has(&["members-only", "join this channel"]) {
        unavailable(tr("Channel members only", "Лише для спонсорів каналу", "Только для спонсоров канала"))
    } else if has(&["premium"]) {
        unavailable(tr("YouTube Premium only", "Лише для YouTube Premium", "Только для YouTube Premium"))
    } else if has(&[
        "removed by the uploader",
        "has been removed",
        "account associated with this video has been terminated",
    ]) {
        unavailable(tr("Video removed", "Відео видалено", "Видео удалено"))
    } else if has(&["premieres in", "live event will begin", "this live event"]) {
        unavailable(tr("The stream or premiere hasn't started yet", "Трансляція або прем'єра ще не почалася", "Трансляция или премьера ещё не началась"))
    } else if has(&[
        "video unavailable",
        "video is unavailable",
        "is not available",
        "isn't available",
        "no longer available",
    ]) {
        unavailable(tr("Video unavailable", "Відео недоступне", "Видео недоступно"))
    } else if has(&[
        "sign in",
        "cookies are no longer valid",
        "login required",
        "--cookies",
    ]) {
        YtError::Auth
    } else {
        let line = message.trim();
        let short: String = line.chars().take(300).collect();
        YtError::Tool(if short.is_empty() {
            tr("unknown error", "невідома помилка", "неизвестная ошибка").into()
        } else {
            short
        })
    }
}

fn clone_result(r: &Result<ResolvedStream>) -> Result<ResolvedStream> {
    match r {
        Ok(s) => Ok(s.clone()),
        Err(e) => Err(clone_err(e)),
    }
}

fn clone_err(e: &YtError) -> YtError {
    match e {
        YtError::Network(s) => YtError::Network(s.clone()),
        YtError::Auth => YtError::Auth,
        YtError::RateLimited => YtError::RateLimited,
        YtError::Unavailable(s) => YtError::Unavailable(s.clone()),
        YtError::Parse(s) => YtError::Parse(s.clone()),
        YtError::Tool(s) => YtError::Tool(s.clone()),
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BATCH_JSONL: &str = include_str!("../tests/fixtures/ytdlp_batch.jsonl");
    const NOW: u64 = 1_792_200_000;

    fn ids(list: &[&str]) -> Vec<TrackId> {
        list.iter().map(|s| TrackId::new(*s)).collect()
    }

    fn unavailable(e: &YtError) -> &str {
        match e {
            YtError::Unavailable(s) => s,
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn parses_fixture_batch() {
        let out = RunOutput {
            stdout: BATCH_JSONL.to_owned(),
            stderr: "ERROR: [youtube] xxxxxxxxxxx: Video unavailable. This video is unavailable\n"
                .into(),
            failure: None,
        };
        let res = attribute(
            &ids(&["dQw4w9WgXcQ", "xxxxxxxxxxx", "aaaaaaaaaaa"]),
            &out,
            NOW,
        );
        assert_eq!(res.len(), 3);

        let opus = res[0].1.as_ref().unwrap();
        assert_eq!(res[0].0.as_str(), "dQw4w9WgXcQ");
        assert_eq!(opus.itag, 251);
        assert_eq!(opus.container, Container::Webm);
        assert_eq!(opus.container.as_str(), "webm");
        assert_eq!(opus.size, Some(3_433_755));
        assert_eq!(opus.duration_ms, 213_000);
        assert_eq!(opus.expires_at, 1_792_223_306);
        assert!(
            opus.url
                .starts_with("https://rr1---sn-test.googlevideo.com/videoplayback?")
        );

        assert_eq!(
            unavailable(res[1].1.as_ref().unwrap_err()),
            "Video unavailable"
        );

        // No top-level url: taken from requested_downloads, size from filesize_approx.
        let aac = res[2].1.as_ref().unwrap();
        assert_eq!(aac.itag, 140);
        assert_eq!(aac.container, Container::M4a);
        assert_eq!(aac.size, Some(2_000_000));
        assert_eq!(aac.duration_ms, 180_500);
        assert_eq!(aac.expires_at, NOW + FALLBACK_TTL_SECS);
    }

    #[test]
    fn missing_ids_get_failure_or_general_error() {
        let ids = ids(&["dQw4w9WgXcQ", "bbbbbbbbbbb"]);
        let timeout = RunOutput::failed("no answer in 30 s".into());
        for (_, r) in attribute(&ids, &timeout, NOW) {
            assert!(matches!(r, Err(YtError::Tool(ref s)) if s.contains("30 s")));
        }
        let general = RunOutput {
            stderr: "ERROR: Unable to download API page: HTTP Error 429: Too Many Requests\n"
                .into(),
            ..RunOutput::default()
        };
        for (_, r) in attribute(&ids, &general, NOW) {
            assert!(matches!(r, Err(YtError::RateLimited)));
        }
        let crash = RunOutput {
            stderr: "Traceback (most recent call last):\n  ...\nKeyError: 'formats'\n".into(),
            ..RunOutput::default()
        };
        let res = attribute(&ids, &crash, NOW);
        assert!(matches!(&res[0].1, Err(YtError::Tool(s)) if s == "KeyError: 'formats'"));
    }

    #[test]
    fn error_lines_are_attributed() {
        assert_eq!(
            parse_error_line("ERROR: [youtube] dQw4w9WgXcQ: Private video. Sign in"),
            Some((Some("dQw4w9WgXcQ"), "Private video. Sign in"))
        );
        assert_eq!(
            parse_error_line("ERROR: [youtube:tab] Something odd: here"),
            Some((None, "Something odd: here"))
        );
        assert_eq!(
            parse_error_line("ERROR: plain failure"),
            Some((None, "plain failure"))
        );
        assert_eq!(parse_error_line("WARNING: ignored"), None);
    }

    #[test]
    fn classifies_errors() {
        let cases = [
            (
                "Sign in to confirm you're not a bot. Use --cookies-from-browser",
                None,
            ),
            ("Sign in to confirm you\u{2019}re not a bot", None),
            (
                "Unable to download webpage: HTTP Error 429: Too Many Requests",
                None,
            ),
            (
                "This content isn't available, try again later. Your account has been rate-limited",
                None,
            ),
            (
                "Video unavailable. This video contains content from UMG, who has blocked it in your country on copyright grounds",
                Some("Blocked by the copyright holder"),
            ),
            (
                "This video is no longer available due to a copyright claim by SomeLabel",
                Some("Blocked by the copyright holder"),
            ),
            (
                "Video unavailable. The uploader has not made this video available in your country",
                Some("Not available in your country"),
            ),
            (
                "Requested format is not available. Use --list-formats",
                Some("No suitable audio track"),
            ),
            (
                "Private video. Sign in if you've been granted access to this video",
                Some("Private video"),
            ),
            (
                "Join this channel to get access to members-only content like this video",
                Some("Channel members only"),
            ),
            (
                "This video is only available to Music Premium members",
                Some("YouTube Premium only"),
            ),
            (
                "Video unavailable. This video has been removed by the uploader",
                Some("Video removed"),
            ),
            (
                "Premieres in 3 hours",
                Some("The stream or premiere hasn't started yet"),
            ),
            ("Video unavailable", Some("Video unavailable")),
            ("This video is not available", Some("Video unavailable")),
        ];
        for (msg, want) in cases {
            match (classify(msg), want) {
                (YtError::RateLimited, None) => {}
                (YtError::Unavailable(got), Some(want)) => assert_eq!(got, want, "{msg}"),
                (other, _) => panic!("{msg}: unexpected {other:?}"),
            }
        }
        assert!(matches!(
            classify("Sign in to confirm your age"),
            YtError::Auth
        ));
        assert!(matches!(
            classify("The provided YouTube account cookies are no longer valid"),
            YtError::Auth
        ));
        assert!(
            matches!(classify("Some new breakage"), YtError::Tool(s) if s == "Some new breakage")
        );
    }

    #[test]
    fn parses_expire() {
        assert_eq!(
            parse_expire("https://x.googlevideo.com/videoplayback?expire=1792223306&ei=abc"),
            Some(1_792_223_306)
        );
        assert_eq!(
            parse_expire("https://x.googlevideo.com/videoplayback?ei=abc&expire=17&id=1"),
            Some(17)
        );
        assert_eq!(
            parse_expire("https://x.googlevideo.com/videoplayback/expire/1700000000/ei/abc"),
            Some(1_700_000_000)
        );
        assert_eq!(
            parse_expire("https://x.googlevideo.com/videoplayback?ei=abc&noexpire=5"),
            None
        );
        assert_eq!(
            parse_expire("https://x.googlevideo.com/videoplayback?expire=soon"),
            None
        );
    }

    fn stream(expires_at: u64) -> ResolvedStream {
        ResolvedStream {
            url: "https://example.invalid/a".into(),
            itag: 251,
            container: Container::Webm,
            size: None,
            duration_ms: 1000,
            expires_at,
        }
    }

    fn scratch_resolver() -> StreamResolver {
        let dir = std::env::temp_dir().join(format!("mixtape-yt-test-{}", fastrand::u64(..)));
        StreamResolver::new("yt-dlp.exe".into(), "deno.exe".into(), dir)
    }

    #[test]
    fn cache_respects_expiry_margin_and_invalidate() {
        let r = scratch_resolver();
        let id = TrackId::new("dQw4w9WgXcQ");
        r.cache_put(id.clone(), stream(NOW + 3600));
        assert!(r.cache_get(&id, NOW).is_some());
        // Within ten minutes of expiry the URL is no longer handed out, and is dropped.
        assert!(r.cache_get(&id, NOW + 3600 - 600).is_none());
        assert!(r.cache_get(&id, NOW).is_none());

        r.cache_put(id.clone(), stream(NOW + 3600));
        r.invalidate(&id);
        assert!(r.cache_get(&id, NOW).is_none());
    }

    #[test]
    fn debug_hides_cookies() {
        let r = scratch_resolver();
        r.set_cookies(Some(
            ".youtube.com\tTRUE\t/\tTRUE\t0\tSECRETNAME\tsecretvalue".into(),
        ));
        let s = format!("{r:?}");
        assert!(s.contains("has_cookies: true"));
        assert!(!s.contains("secretvalue"));
    }

    /// A temp dir with a fake `yt-dlp.cmd` that prints the fixture, reports one error and
    /// copies the cookie file it got (argument 2) aside.
    #[cfg(windows)]
    fn fake_ytdlp(body: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mixtape-yt-fake-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("batch.jsonl"), BATCH_JSONL).unwrap();
        let script = dir.join("yt-dlp.cmd");
        std::fs::write(&script, format!("@echo off\r\n{body}\r\n")).unwrap();
        (dir, script)
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn runs_process_with_temporary_cookie_copy() {
        let (dir, script) = fake_ytdlp(
            "copy /y \"%~4\" \"%~dp0seen.txt\" >nul\r\n\
             type \"%~dp0batch.jsonl\"\r\n\
             echo ERROR: [youtube] xxxxxxxxxxx: Private video. Sign in 1>&2\r\n\
             exit /b 1",
        );
        let tmp = dir.join("tmp");
        let r = StreamResolver::new(script, "deno.exe".into(), tmp.clone());
        r.set_cookies(Some(".youtube.com\tTRUE\t/\tTRUE\t0\tNAME\tfake".into()));

        let res = r
            .resolve(&ids(&["dQw4w9WgXcQ", "xxxxxxxxxxx", "dQw4w9WgXcQ"]))
            .await;
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].1.as_ref().unwrap().itag, 251);
        assert_eq!(
            unavailable(res[1].1.as_ref().unwrap_err()),
            "Private video"
        );
        assert_eq!(res[2].1.as_ref().unwrap().itag, 251);

        let seen = std::fs::read_to_string(dir.join("seen.txt")).unwrap();
        assert!(seen.starts_with(NETSCAPE_HEADER));
        assert!(seen.contains("\tNAME\tfake"));
        let left = std::fs::read_dir(&tmp).unwrap().count();
        assert_eq!(left, 0, "cookie copy must not outlive the call");

        // Cached now: a second call does not need the process. The fixture URL carries a real
        // `expire`, so once that date passes the entry counts as stale.
        let fresh = res[0].1.as_ref().unwrap().expires_at > now_unix() + EXPIRY_MARGIN_SECS;
        assert_eq!(r.cached(&TrackId::new("dQw4w9WgXcQ")).is_some(), fresh);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Logs its arguments, then answers after about a second so concurrent calls overlap.
    #[cfg(windows)]
    const SLOW_LOGGING: &str = "echo %* >> \"%~dp0calls.txt\"\r\n\
                                ping -n 2 127.0.0.1 >nul\r\n\
                                type \"%~dp0batch.jsonl\"";

    #[cfg(windows)]
    #[tokio::test]
    async fn concurrent_calls_share_one_process_per_id() {
        let (dir, script) = fake_ytdlp(SLOW_LOGGING);
        let r = StreamResolver::new(script, "deno.exe".into(), dir.join("tmp"));
        let id = TrackId::new("dQw4w9WgXcQ");
        let batch = ids(&["aaaaaaaaaaa", "dQw4w9WgXcQ"]);
        let (a, b, c) = tokio::join!(r.resolve_one(&id), r.resolve_one(&id), r.resolve(&batch));
        assert_eq!(a.unwrap().itag, 251);
        assert_eq!(b.unwrap().itag, 251);
        assert_eq!(c[0].1.as_ref().unwrap().itag, 140);
        assert_eq!(c[1].1.as_ref().unwrap().itag, 251);

        let calls = std::fs::read_to_string(dir.join("calls.txt")).unwrap();
        assert_eq!(calls.lines().count(), 2, "one process per new id: {calls}");
        assert_eq!(calls.matches("dQw4w9WgXcQ").count(), 1, "{calls}");
        assert_eq!(calls.matches("aaaaaaaaaaa").count(), 1, "{calls}");
        assert!(lock(&r.inflight).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn dropped_call_lets_waiters_retry() {
        let (dir, script) = fake_ytdlp(SLOW_LOGGING);
        let r = StreamResolver::new(script, "deno.exe".into(), dir.join("tmp"));
        let id = TrackId::new("dQw4w9WgXcQ");
        let first = tokio::time::timeout(Duration::from_millis(300), r.resolve_one(&id));
        let waiter = async {
            // Joins once the first call has claimed the id.
            tokio::task::yield_now().await;
            assert!(lock(&r.inflight).contains_key(&id));
            r.resolve_one(&id).await
        };
        let (first, waited) = tokio::join!(first, waiter);
        assert!(first.is_err(), "the first call is cut off");
        assert_eq!(waited.unwrap().itag, 251);
        assert!(lock(&r.inflight).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The script starts a grandchild that writes `late.txt` after ~3 s; dropping the call
    /// must kill it too, not only the direct child (`kill_on_drop`).
    #[cfg(windows)]
    #[tokio::test]
    async fn dropped_call_kills_the_process_tree() {
        let (dir, script) = fake_ytdlp(
            "start \"\" /b cmd /c \"ping -n 4 127.0.0.1 >nul & echo late > \"%~dp0late.txt\"\"\r\n\
             ping -n 30 127.0.0.1 >nul",
        );
        let r = StreamResolver::new(script, "deno.exe".into(), dir.join("tmp"));
        let id = TrackId::new("dQw4w9WgXcQ");
        let call = tokio::time::timeout(Duration::from_millis(700), r.resolve_one(&id));
        assert!(call.await.is_err(), "the call is cut off");
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!dir.join("late.txt").exists(), "a grandchild outlived the call");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn hardened_arguments_and_bad_ids() {
        let (dir, script) = fake_ytdlp(SLOW_LOGGING);
        let r = StreamResolver::new(script, "deno.exe".into(), dir.join("tmp"));
        let res = r.resolve(&ids(&["dQw4w9WgXcQ", "../evil"])).await;
        assert_eq!(res[0].1.as_ref().unwrap().itag, 251);
        assert!(matches!(res[1].1, Err(YtError::Unavailable(_))), "{:?}", res[1].1);
        let calls = std::fs::read_to_string(dir.join("calls.txt")).unwrap();
        assert!(calls.starts_with("--ignore-config --no-plugin-dirs "), "{calls}");
        assert!(calls.contains(" -- \"https://www.youtube.com/watch?v=dQw4w9WgXcQ\""), "{calls}");
        assert!(!calls.contains("evil"), "{calls}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_put_prunes_stale_entries() {
        let r = scratch_resolver();
        let now = now_unix();
        r.cache_put(TrackId::new("aaaaaaaaaaa"), stream(now + 60));
        r.cache_put(TrackId::new("bbbbbbbbbbb"), stream(now + 3600));
        r.cache_put(TrackId::new("ccccccccccc"), stream(now + 7200));
        let cache = lock(&r.cache);
        assert_eq!(cache.len(), 2);
        assert!(!cache.contains_key(&TrackId::new("aaaaaaaaaaa")));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timeout_kills_the_process_tree() {
        let (dir, script) = fake_ytdlp("ping -n 30 127.0.0.1 >nul");
        let r = StreamResolver::new(script, "deno.exe".into(), dir.join("tmp"))
            .with_timeout(Duration::from_millis(800));
        r.set_cookies(Some("x".into()));
        let started = Instant::now();
        let err = r
            .resolve_one(&TrackId::new("dQw4w9WgXcQ"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, YtError::Tool(ref s) if s.contains("no answer")),
            "{err:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_binary_is_a_tool_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let r = StreamResolver::new(
            "Z:/nope/yt-dlp.exe".into(),
            "deno.exe".into(),
            std::env::temp_dir(),
        );
        let err = rt
            .block_on(r.resolve_one(&TrackId::new("dQw4w9WgXcQ")))
            .unwrap_err();
        assert!(matches!(err, YtError::Tool(ref s) if s.contains("couldn't start")));
    }

    /// Live: `MIXTAPE_SIDECARS=<dir with yt-dlp/deno> cargo test -p mixtape-yt -- --ignored live_`.
    /// `MIXTAPE_COOKIES=<cookies.txt>` resolves with cookies. Fetches 1 KiB, never plays.
    #[tokio::test]
    #[ignore]
    async fn live_resolve_public_video() {
        let dir = std::env::var_os("MIXTAPE_SIDECARS")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/binaries")
            });
        let tmp = std::env::temp_dir().join(format!("mixtape-yt-live-{}", fastrand::u64(..)));
        let r = StreamResolver::new(
            dir.join("yt-dlp-x86_64-pc-windows-msvc.exe"),
            dir.join("deno-x86_64-pc-windows-msvc.exe"),
            tmp.clone(),
        );
        if let Some(path) = std::env::var_os("MIXTAPE_COOKIES") {
            r.set_cookies(Some(std::fs::read_to_string(path).unwrap()));
        }
        let s = r.resolve_one(&TrackId::new("dQw4w9WgXcQ")).await.unwrap();
        assert!(s.itag == 251 || s.itag == 140);
        assert!(s.expires_at > now_unix() + 3600);
        assert!(s.duration_ms > 200_000);
        let resp = reqwest::Client::new()
            .get(&s.url)
            .header("Range", "bytes=0-1023")
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{}", resp.status());
        assert_eq!(resp.bytes().await.unwrap().len(), 1024);
        if tmp.exists() {
            assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        }
        std::fs::remove_dir_all(&tmp).ok();
    }
}
