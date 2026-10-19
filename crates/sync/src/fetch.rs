//! Chunked file download with resume.
//!
//! The file is fetched in [`CHUNK`]-sized pieces. Each request carries both the
//! googlevideo-style `range=start-end` URL parameter and a standard `Range` header, so
//! YouTube stream URLs and ordinary HTTP servers both answer with just that piece.
//! Bytes are appended to `<name>.part` as they arrive; a later call resumes from the
//! bytes already there. [`finalize`] moves the finished file into place.

use mixtape_core::tr;
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
    #[error("{}", tr("The audio link expired", "Посилання на аудіо застаріло", "ссылка на аудио устарела"))]
    Expired,
    #[error("{} {}", tr("The server answered with error", "Сервер відповів помилкою", "сервер ответил ошибкой"), .0)]
    Http(u16),
    #[error("{}: {}", tr("Network error", "Помилка мережі", "ошибка сети"), .0)]
    Network(String),
    #[error("{}: {}", tr("Couldn't write the file", "Помилка запису файлу", "ошибка записи файла"), .0)]
    Io(#[from] std::io::Error),
    #[error("{}", tr("Download stopped", "Завантаження зупинено", "загрузка остановлена"))]
    Cancelled,
}

impl From<reqwest::Error> for FetchError {
    fn from(e: reqwest::Error) -> Self {
        match e.status() {
            Some(StatusCode::FORBIDDEN) => Self::Expired,
            Some(s) => Self::Http(s.as_u16()),
            None => Self::Network(network_text(&e)),
        }
    }
}

/// A network failure in the user's language. reqwest's own text is English and carries
/// the request URL — for googlevideo that holds the user's IP — so it is never shown.
fn network_text(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        tr("the server didn't answer in time", "сервер не відповів вчасно", "сервер не ответил вовремя")
    } else if e.is_connect() {
        tr("no connection to the server", "немає з'єднання з сервером", "нет соединения с сервером")
    } else if e.is_body() || e.is_decode() {
        tr("the connection broke off", "з'єднання обірвалося", "соединение оборвалось")
    } else {
        tr("the request failed", "запит не вдався", "запрос не удался")
    }
    .into()
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
        let req = ranged(client, url, start, end).send();
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
            return Err(FetchError::Network(
                tr("the server sent an empty answer", "сервер надіслав порожню відповідь", "сервер прислал пустой ответ").into(),
            ));
        }
    }
}

/// A request for bytes `start..=end`. googlevideo gets only its `range` parameter: with a
/// `Range` header as well it applies the header to the already cut piece and answers 416
/// for any piece not starting at 0. Other servers get the standard header.
pub(crate) fn ranged(client: &reqwest::Client, url: &str, start: u64, end: u64) -> reqwest::RequestBuilder {
    let googlevideo = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.ends_with(".googlevideo.com")))
        .unwrap_or(false);
    if googlevideo {
        client.get(with_range(url, start, end))
    } else {
        client.get(url).header(RANGE, format!("bytes={start}-{end}"))
    }
}

/// Appends the googlevideo `range` parameter.
pub(crate) fn with_range(url: &str, start: u64, end: u64) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}range={start}-{end}")
}

/// The exact size of the file behind `url`: its `clen` parameter, else the total of a
/// one-byte ranged request. `Err(Expired)` on HTTP 403.
pub async fn exact_len(client: &reqwest::Client, url: &str) -> Result<u64, FetchError> {
    if let Some(n) = clen_param(url) {
        return Ok(n);
    }
    // A `Range` header alone: googlevideo answers it with the total in `Content-Range`.
    let resp = client
        .get(url)
        .header(RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;
    match resp.status() {
        StatusCode::FORBIDDEN => return Err(FetchError::Expired),
        s if !s.is_success() => return Err(FetchError::Http(s.as_u16())),
        _ => {}
    }
    resp.headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(content_range_total)
        .or(resp.content_length().filter(|&n| n > 1))
        .ok_or(FetchError::Http(resp.status().as_u16()))
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

/// What a `.part` file holds: the stream format and its exact length (`clen`). A resume
/// appends to the bytes already there, so it must fetch the very same file.
pub fn part_key(itag: u32, url: &str) -> String {
    match clen_param(url) {
        Some(n) => format!("{itag}:{n}"),
        None => itag.to_string(),
    }
}

fn key_path(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_owned();
    s.push(".src");
    PathBuf::from(s)
}

/// Makes `part` belong to `key` before a fetch: a `.part` left by another format (or by a
/// version that did not record one) is thrown away instead of resumed.
pub fn claim_part(part: &Path, key: &str) -> std::io::Result<()> {
    let marker = key_path(part);
    let same = std::fs::read_to_string(&marker).is_ok_and(|k| k == key);
    if !same {
        match std::fs::remove_file(part) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        if let Some(dir) = marker.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&marker, key)?;
    }
    Ok(())
}

/// Moves a finished `part` file to `final_path`, creating folders and replacing an
/// existing file.
pub fn finalize(part: &Path, final_path: &Path) -> std::io::Result<()> {
    if let Some(dir) = final_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(part, final_path)?;
    let _ = std::fs::remove_file(key_path(part));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::{Mode, Server, data, serve};
    use std::time::Duration;

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
    async fn network_errors_hide_the_url() {
        // Nothing listens on port 9 of localhost: a connect error.
        let url = "http://127.0.0.1:9/videoplayback?ip=203.0.113.7&itag=251";
        let e = reqwest::Client::new().get(url).send().await.unwrap_err();
        let shown = FetchError::from(e).to_string();
        assert!(!shown.contains("203.0.113.7") && !shown.contains("127.0.0.1"), "{shown}");
        assert!(!shown.contains("error sending request"), "{shown}");
    }

    #[test]
    fn a_part_of_another_format_is_not_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.webm.part");
        let url = "https://x.googlevideo.com/videoplayback?itag=251&clen=5000";
        assert_eq!(part_key(251, url), "251:5000");
        // Left by a version that did not record its format: dropped.
        std::fs::write(&part, b"old").unwrap();
        claim_part(&part, &part_key(251, url)).unwrap();
        assert!(!part.exists());
        // Same format again: kept for the resume.
        std::fs::write(&part, b"abc").unwrap();
        claim_part(&part, &part_key(251, url)).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), b"abc");
        // A fresh URL for another format: dropped.
        claim_part(&part, &part_key(250, url)).unwrap();
        assert!(!part.exists());
        std::fs::write(&part, b"done").unwrap();
        finalize(&part, &dir.path().join("a.webm")).unwrap();
        assert!(!key_path(&part).exists());
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
        assert_eq!(err.to_string(), "The audio link expired");
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
    fn googlevideo_gets_only_the_range_parameter() {
        let c = reqwest::Client::new();
        let g = ranged(&c, "https://rr1---sn-x.googlevideo.com/videoplayback?a=1", 100, 199).build().unwrap();
        assert!(g.url().as_str().ends_with("&range=100-199"));
        assert!(g.headers().get(RANGE).is_none(), "both together answer 416 past 0");
        let o = ranged(&c, "http://127.0.0.1:9/f", 100, 199).build().unwrap();
        assert_eq!(o.headers().get(RANGE).unwrap(), "bytes=100-199");
        assert!(o.url().query().is_none());
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
