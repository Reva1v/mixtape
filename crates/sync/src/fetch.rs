//! Chunked file download with resume.
//!
//! The file is fetched in [`CHUNK`]-sized pieces. Each request carries both the
//! googlevideo-style `range=start-end` URL parameter and a standard `Range` header, so
//! YouTube stream URLs and ordinary HTTP servers both answer with just that piece.
//! Bytes are appended to `<name>.part` as they arrive; a later call resumes from the
//! bytes already there. [`finalize`] moves the finished file into place.

use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Notify;

/// Size of one ranged request (10 MiB), what YouTube serves without throttling.
pub const CHUNK: u64 = 10 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// HTTP 403: the stream URL expired; the caller resolves a fresh one and retries
    /// (the `.part` file is kept).
    #[error("ссылка на аудио устарела")]
    Expired,
    #[error("сервер ответил ошибкой {0}")]
    Http(u16),
    #[error("ошибка сети: {0}")]
    Network(String),
    #[error("ошибка записи файла: {0}")]
    Io(#[from] std::io::Error),
    #[error("загрузка остановлена")]
    Cancelled,
}

impl From<reqwest::Error> for FetchError {
    fn from(e: reqwest::Error) -> Self {
        match e.status() {
            Some(StatusCode::FORBIDDEN) => Self::Expired,
            Some(s) => Self::Http(s.as_u16()),
            None => Self::Network(e.to_string()),
        }
    }
}

/// Cancellation flag shared between a download and whoever stops it. Cloning shares
/// the same flag.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<CancelInner>);

#[derive(Debug, Default)]
struct CancelInner {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    /// Completes once [`Cancel::cancel`] has been called.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.0.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// `<final>.part`, where an unfinished download of `final_path` is kept.
pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// Downloads `url` into `part`, resuming from its current length, and returns the
/// final size. `expected` is the full size if known (otherwise the URL's `clen`
/// parameter or the server's `Content-Range` is used; failing both, a short piece
/// marks the end). `on_progress(done, total)` runs after every received block, so the
/// caller throttles UI updates. On cancel or error the bytes received so far stay in
/// `part`. Timeouts come from the `client` configuration.
pub async fn fetch(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    expected: Option<u64>,
    cancel: &Cancel,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, FetchError> {
    fetch_chunked(client, url, part, expected, cancel, CHUNK, on_progress).await
}

async fn fetch_chunked(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    expected: Option<u64>,
    cancel: &Cancel,
    chunk: u64,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, FetchError> {
    if let Some(dir) = part.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(part)
        .await?;
    let mut w = Writer {
        file: &mut file,
        done: 0,
    };
    let res = download(
        &mut w,
        client,
        url,
        expected.or_else(|| clen_param(url)),
        cancel,
        chunk,
        &mut on_progress,
    )
    .await;
    // tokio's File finishes writes in the background; flush so nothing is lost.
    let flushed = file.flush().await;
    let size = res?;
    flushed?;
    Ok(size)
}

struct Writer<'a> {
    file: &'a mut tokio::fs::File,
    done: u64,
}

impl Writer<'_> {
    async fn reset(&mut self) -> std::io::Result<()> {
        self.file.set_len(0).await?;
        self.file.seek(SeekFrom::Start(0)).await?;
        self.done = 0;
        Ok(())
    }
}

async fn download(
    w: &mut Writer<'_>,
    client: &reqwest::Client,
    url: &str,
    mut total: Option<u64>,
    cancel: &Cancel,
    chunk: u64,
    on_progress: &mut impl FnMut(u64, Option<u64>),
) -> Result<u64, FetchError> {
    w.done = w.file.metadata().await?.len();
    if total.is_some_and(|t| w.done > t) {
        w.reset().await?;
    } else {
        w.file.seek(SeekFrom::Start(w.done)).await?;
    }
    on_progress(w.done, total);

    loop {
        if total.is_some_and(|t| w.done >= t) {
            return Ok(w.done);
        }
        if cancel.is_cancelled() {
            return Err(FetchError::Cancelled);
        }
        let start = w.done;
        let end = match total {
            Some(t) => (start + chunk).min(t) - 1,
            None => start + chunk - 1,
        };
        let want = end - start + 1;
        let req = client
            .get(with_range(url, start, end))
            .header(RANGE, format!("bytes={start}-{end}"))
            .send();
        let mut resp = tokio::select! {
            r = req => r?,
            _ = cancel.cancelled() => return Err(FetchError::Cancelled),
        };
        let status = resp.status();
        if status == StatusCode::FORBIDDEN {
            return Err(FetchError::Expired);
        }
        if status == StatusCode::RANGE_NOT_SATISFIABLE && total.is_none() && start > 0 {
            // Asked past the end of a file of unknown size: it is complete.
            return Ok(w.done);
        }
        if !status.is_success() {
            return Err(FetchError::Http(status.as_u16()));
        }

        let content_range = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok());
        if let Some(t) = content_range.and_then(content_range_total) {
            total = Some(t);
        }
        // A plain 200 bigger than the piece means the server ignores ranges and sends
        // the whole file: start over and take it in one go.
        let whole = status == StatusCode::OK
            && content_range.is_none()
            && resp.content_length().is_some_and(|n| n > want);
        if whole {
            total = resp.content_length();
            if start > 0 {
                w.reset().await?;
            }
        }

        let mut got = 0u64;
        loop {
            let next = tokio::select! {
                c = resp.chunk() => c?,
                _ = cancel.cancelled() => return Err(FetchError::Cancelled),
            };
            let Some(bytes) = next else { break };
            w.file.write_all(&bytes).await?;
            got += bytes.len() as u64;
            w.done += bytes.len() as u64;
            on_progress(w.done, total);
        }

        if whole {
            return Ok(w.done);
        }
        if got < want && total.is_none() {
            // A short piece of a file of unknown size is its tail.
            return Ok(w.done);
        }
        if got == 0 {
            return Err(FetchError::Network("сервер прислал пустой ответ".into()));
        }
    }
}

/// Appends the googlevideo `range` parameter.
fn with_range(url: &str, start: u64, end: u64) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}range={start}-{end}")
}

/// The `clen` (content length) parameter of a googlevideo URL.
fn clen_param(url: &str) -> Option<u64> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "clen")
        .and_then(|(_, v)| v.parse().ok())
}

/// Total size from `Content-Range: bytes a-b/total`.
fn content_range_total(v: &str) -> Option<u64> {
    v.rsplit_once('/').and_then(|(_, t)| t.trim().parse().ok())
}

/// Moves a finished `part` file to `final_path`, creating folders and replacing an
/// existing file.
pub fn finalize(part: &Path, final_path: &Path) -> std::io::Result<()> {
    if let Some(dir) = final_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(part, final_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[derive(Clone, Copy)]
    enum Mode {
        /// 200 with only the requested range, like googlevideo with `&range=`.
        Param,
        /// 206 with `Content-Range`, like a standard server.
        Partial,
        /// Ignores ranges and always sends the whole file.
        Whole,
        Forbidden,
        /// Sends the headers and `n` bytes of the piece, then hangs.
        Stall(usize),
    }

    struct Server {
        base: String,
        /// Requested `(start, end)` per request.
        ranges: Arc<Mutex<Vec<(u64, u64)>>>,
    }

    fn data(n: usize) -> Arc<Vec<u8>> {
        Arc::new((0..n).map(|i| (i * 31 % 251) as u8).collect())
    }

    fn parse_range(head: &str) -> Option<(u64, u64)> {
        let path = head.lines().next()?.split_whitespace().nth(1)?;
        let from_param = path
            .split(['?', '&'])
            .find_map(|kv| kv.strip_prefix("range="))
            .map(str::to_owned);
        let from_header = head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("range")
                .then(|| v.trim().trim_start_matches("bytes=").to_owned())
        });
        let r = from_param.or(from_header)?;
        let (a, b) = r.split_once('-')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }

    async fn serve(body: Arc<Vec<u8>>, mode: Mode) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!(
            "http://{}/audio?clen_unknown=1",
            listener.local_addr().unwrap()
        );
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let log = ranges.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let body = body.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut tmp).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf).into_owned();
                    let len = body.len() as u64;
                    let range = parse_range(&head);
                    if let Some(r) = range {
                        log.lock().unwrap().push(r);
                    }
                    let (a, b) = range.unwrap_or((0, len.saturating_sub(1)));
                    let reply = |status: &str, extra: String, payload: &[u8]| {
                        let mut v = format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                            payload.len()
                        )
                        .into_bytes();
                        v.extend_from_slice(payload);
                        v
                    };
                    let out = match mode {
                        Mode::Forbidden => reply("403 Forbidden", String::new(), b""),
                        Mode::Whole => reply("200 OK", String::new(), &body),
                        _ if a >= len => reply("416 Range Not Satisfiable", String::new(), b""),
                        Mode::Param => reply(
                            "200 OK",
                            String::new(),
                            &body[a as usize..=(b.min(len - 1)) as usize],
                        ),
                        Mode::Partial => {
                            let b = b.min(len - 1);
                            reply(
                                "206 Partial Content",
                                format!("Content-Range: bytes {a}-{b}/{len}\r\n"),
                                &body[a as usize..=b as usize],
                            )
                        }
                        Mode::Stall(n) => {
                            let piece = &body[a as usize..=(b.min(len - 1)) as usize];
                            let mut v = reply("200 OK", String::new(), piece);
                            v.truncate(v.len() - piece.len() + n);
                            let _ = sock.write_all(&v).await;
                            let _ = sock.flush().await;
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            return;
                        }
                    };
                    let _ = sock.write_all(&out).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Server { base, ranges }
    }

    async fn run(
        s: &Server,
        part: &Path,
        expected: Option<u64>,
        chunk: u64,
    ) -> Result<u64, FetchError> {
        let client = reqwest::Client::new();
        fetch_chunked(
            &client,
            &s.base,
            part,
            expected,
            &Cancel::new(),
            chunk,
            |_, _| {},
        )
        .await
    }

    #[tokio::test]
    async fn full_download_in_pieces() {
        let body = data(100_000);
        let dir = tempfile::tempdir().unwrap();
        for (mode, expected) in [
            (Mode::Param, Some(100_000)),
            (Mode::Param, None),
            (Mode::Partial, None),
            (Mode::Whole, None),
        ] {
            let s = serve(body.clone(), mode).await;
            let part = dir.path().join("x/a.webm.part");
            let _ = std::fs::remove_file(&part);
            let n = run(&s, &part, expected, 16_384).await.unwrap();
            assert_eq!(n, 100_000);
            assert_eq!(std::fs::read(&part).unwrap(), *body);
        }
    }

    #[tokio::test]
    async fn pieces_follow_the_range() {
        let s = serve(data(40_000), Mode::Param).await;
        let dir = tempfile::tempdir().unwrap();
        run(&s, &dir.path().join("a.part"), Some(40_000), 16_384)
            .await
            .unwrap();
        assert_eq!(
            *s.ranges.lock().unwrap(),
            vec![(0, 16_383), (16_384, 32_767), (32_768, 39_999)]
        );
    }

    #[tokio::test]
    async fn resumes_from_part() {
        let body = data(50_000);
        let s = serve(body.clone(), Mode::Param).await;
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.part");
        std::fs::write(&part, &body[..30_000]).unwrap();
        let mut seen = Vec::new();
        let client = reqwest::Client::new();
        let n = fetch_chunked(
            &client,
            &s.base,
            &part,
            Some(50_000),
            &Cancel::new(),
            16_384,
            |d, t| seen.push((d, t)),
        )
        .await
        .unwrap();
        assert_eq!(n, 50_000);
        assert_eq!(std::fs::read(&part).unwrap(), *body);
        assert_eq!(s.ranges.lock().unwrap()[0].0, 30_000);
        assert_eq!(seen.first(), Some(&(30_000, Some(50_000))));
        assert_eq!(seen.last(), Some(&(50_000, Some(50_000))));

        // Already complete: no request at all.
        let before = s.ranges.lock().unwrap().len();
        assert_eq!(run(&s, &part, Some(50_000), 16_384).await.unwrap(), 50_000);
        assert_eq!(s.ranges.lock().unwrap().len(), before);
    }

    #[tokio::test]
    async fn oversized_part_restarts() {
        let body = data(10_000);
        let s = serve(body.clone(), Mode::Param).await;
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.part");
        std::fs::write(&part, vec![7u8; 12_000]).unwrap();
        assert_eq!(run(&s, &part, Some(10_000), 4096).await.unwrap(), 10_000);
        assert_eq!(std::fs::read(&part).unwrap(), *body);
    }

    #[tokio::test]
    async fn forbidden_means_expired() {
        let s = serve(data(1000), Mode::Forbidden).await;
        let dir = tempfile::tempdir().unwrap();
        let err = run(&s, &dir.path().join("a.part"), None, 4096)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Expired), "{err:?}");
        assert_eq!(err.to_string(), "ссылка на аудио устарела");
    }

    #[tokio::test]
    async fn cancel_stops_and_keeps_part() {
        let s = serve(data(100_000), Mode::Stall(5000)).await;
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.part");
        let cancel = Cancel::new();
        let trigger = cancel.clone();
        let client = reqwest::Client::new();
        let fut = fetch_chunked(
            &client,
            &s.base,
            &part,
            Some(100_000),
            &cancel,
            65_536,
            |d, _| {
                if d >= 5000 {
                    trigger.cancel();
                }
            },
        );
        let res = tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .expect("cancel was not prompt");
        assert!(matches!(res, Err(FetchError::Cancelled)), "{res:?}");
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 5000);

        // Cancelling from outside while the request hangs.
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.cancel();
        });
        let fut = fetch_chunked(
            &client,
            &s.base,
            &part,
            Some(100_000),
            &cancel,
            65_536,
            |_, _| {},
        );
        let res = tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .expect("cancel was not prompt");
        assert!(matches!(res, Err(FetchError::Cancelled)));
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 10_000);
    }

    #[test]
    fn helpers() {
        assert_eq!(
            with_range("http://h/v?a=1", 0, 9),
            "http://h/v?a=1&range=0-9"
        );
        assert_eq!(with_range("http://h/v", 5, 9), "http://h/v?range=5-9");
        assert_eq!(
            clen_param("https://r1.googlevideo.com/videoplayback?itag=251&clen=3456789"),
            Some(3_456_789)
        );
        assert_eq!(clen_param("https://h/v?itag=251"), None);
        assert_eq!(content_range_total("bytes 0-9/1234"), Some(1234));
        assert_eq!(content_range_total("bytes 0-9/*"), None);
        assert_eq!(
            part_path(Path::new("lib/a.webm")),
            PathBuf::from("lib/a.webm.part")
        );
    }

    #[test]
    fn finalize_moves_into_new_folder() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.webm.part");
        std::fs::write(&part, b"abc").unwrap();
        let dest = dir.path().join("library/x/a.webm");
        finalize(&part, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
        assert!(!part.exists());
        // Replaces an existing file.
        std::fs::write(&part, b"new").unwrap();
        finalize(&part, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
    }
}
