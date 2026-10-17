//! Streams a track into a [`SparseCacheFile`] while it plays.
//!
//! Ranged requests of [`PIECE`] bytes start where the reader is
//! ([`SparseCacheFile::wanted`]); when it seeks elsewhere the current request is dropped and
//! the next one starts there. After the end, the holes that seeks left behind are filled and the
//! task ends. Requests carry both the googlevideo `range` parameter and a `Range` header, as in
//! [`crate::fetch`].

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use mixtape_audio::SparseCacheFile;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_RANGE, RANGE};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::fetch::{Cancel, FetchError, with_range};

/// Size of one ranged request (1 MiB): small enough to retarget quickly after a seek.
pub const PIECE: u64 = 1024 * 1024;
/// Network errors in a row that are retried before giving up.
const RETRIES: u32 = 3;

/// A running stream download; dropping it cancels the download.
pub struct StreamFill {
    cancel: Cancel,
    url: watch::Sender<String>,
    task: Option<JoinHandle<Result<(), FetchError>>>,
}

impl StreamFill {
    /// Starts downloading `url` into `file` on the current Tokio runtime. On HTTP 403
    /// `on_expired` runs and the download waits for [`StreamFill::retarget`] with a fresh URL.
    /// Other failures end it and reach the file's readers through [`SparseCacheFile::fail`].
    /// Timeouts come from the `client` configuration.
    pub fn start(
        client: reqwest::Client,
        url: String,
        file: Arc<SparseCacheFile>,
        on_expired: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_piece(client, url, file, on_expired, PIECE)
    }

    fn with_piece(
        client: reqwest::Client,
        url: String,
        file: Arc<SparseCacheFile>,
        on_expired: impl Fn() + Send + Sync + 'static,
        piece: u64,
    ) -> Self {
        let cancel = Cancel::new();
        let (tx, rx) = watch::channel(url);
        let job = Job {
            client,
            url: rx,
            file,
            cancel: cancel.clone(),
            piece,
        };
        Self {
            cancel,
            url: tx,
            task: Some(tokio::spawn(job.run(on_expired))),
        }
    }

    /// Stops downloading; the file keeps what has arrived.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Continues with a fresh URL of the same file (after `on_expired`, or at any time).
    pub fn retarget(&self, url: String) {
        self.url.send_replace(url);
    }

    /// Whether the download has ended (complete, failed or cancelled).
    pub fn done(&self) -> bool {
        self.task.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits for the end: `Ok` once the file is complete.
    pub async fn wait(mut self) -> Result<(), FetchError> {
        let Some(task) = self.task.take() else {
            return Err(FetchError::Cancelled);
        };
        task.await
            .unwrap_or_else(|e| Err(FetchError::Network(e.to_string())))
    }
}

impl Drop for StreamFill {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct Job {
    client: reqwest::Client,
    url: watch::Receiver<String>,
    file: Arc<SparseCacheFile>,
    cancel: Cancel,
    piece: u64,
}

impl Job {
    async fn run(mut self, on_expired: impl Fn()) -> Result<(), FetchError> {
        let mut failures = 0;
        loop {
            if self.cancel.is_cancelled() || self.file.is_cancelled() {
                return Err(FetchError::Cancelled);
            }
            let Some(gap) = self.file.next_gap(self.file.wanted()) else {
                return Ok(());
            };
            let url = self.url.borrow_and_update().clone();
            match self.fetch_piece(&url, gap).await {
                Ok(()) => failures = 0,
                Err(FetchError::Expired) => {
                    on_expired();
                    tokio::select! {
                        changed = self.url.changed() => {
                            if changed.is_err() {
                                return Err(FetchError::Cancelled);
                            }
                        }
                        _ = self.cancel.cancelled() => return Err(FetchError::Cancelled),
                    }
                }
                Err(FetchError::Network(e)) if failures < RETRIES => {
                    failures += 1;
                    tracing::debug!("stream piece failed ({failures}): {e}");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(500) * failures) => {}
                        _ = self.cancel.cancelled() => return Err(FetchError::Cancelled),
                    }
                }
                Err(FetchError::Cancelled) => return Err(FetchError::Cancelled),
                Err(e) => {
                    self.file.fail(e.to_string());
                    return Err(e);
                }
            }
        }
    }

    /// Downloads the start of `gap`, at most a piece. Returns early once the reader needs a
    /// different hole (it sought away).
    async fn fetch_piece(&self, url: &str, gap: Range<u64>) -> Result<(), FetchError> {
        let (start, end) = (gap.start, gap.end.min(gap.start + self.piece) - 1);
        let req = self
            .client
            .get(with_range(url, start, end))
            .header(RANGE, format!("bytes={start}-{end}"))
            .send();
        let mut resp = tokio::select! {
            r = req => r?,
            _ = self.cancel.cancelled() => return Err(FetchError::Cancelled),
        };
        let status = resp.status();
        if status == StatusCode::FORBIDDEN {
            return Err(FetchError::Expired);
        }
        if !status.is_success() {
            return Err(FetchError::Http(status.as_u16()));
        }
        // A plain 200 bigger than the piece means the server ignores ranges and sends the
        // whole file: take all of it from the start.
        let whole = status == StatusCode::OK
            && !resp.headers().contains_key(CONTENT_RANGE)
            && resp.content_length().is_some_and(|n| n > end - start + 1);
        let (mut pos, limit) = match whole {
            true => (0, self.file.total_len()),
            false => (start, end + 1),
        };
        let mut got = 0u64;
        while pos < limit {
            let next = tokio::select! {
                c = resp.chunk() => c?,
                _ = self.cancel.cancelled() => return Err(FetchError::Cancelled),
            };
            let Some(bytes) = next else { break };
            let n = bytes.len().min((limit - pos) as usize);
            self.file.write_at(pos, &bytes[..n])?;
            pos += n as u64;
            got += n as u64;
            if self.file.is_cancelled() {
                return Err(FetchError::Cancelled);
            }
            if !whole && self.moved(pos) {
                return Ok(());
            }
        }
        if got == 0 {
            return Err(FetchError::Network("сервер прислал пустой ответ".into()));
        }
        Ok(())
    }

    /// Whether the reader's next hole is no longer the one being filled at `pos`. A hole a
    /// quarter piece ahead is reached sooner by this request than by a new one.
    fn moved(&self, pos: u64) -> bool {
        self.file
            .next_gap(self.file.wanted())
            .is_some_and(|g| g.start < pos || g.start > pos + self.piece / 4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::{Mode, data, serve};
    use std::io::{Read, Seek, SeekFrom};

    fn file(dir: &tempfile::TempDir, len: usize) -> Arc<SparseCacheFile> {
        SparseCacheFile::create(dir.path().join("a.webm.part"), len as u64).unwrap()
    }

    fn fill(url: &str, file: &Arc<SparseCacheFile>, piece: u64) -> StreamFill {
        StreamFill::with_piece(
            reqwest::Client::new(),
            url.into(),
            file.clone(),
            || {},
            piece,
        )
    }

    async fn finish(fill: StreamFill) -> Result<(), FetchError> {
        tokio::time::timeout(Duration::from_secs(20), fill.wait())
            .await
            .expect("the download hangs")
    }

    #[tokio::test]
    async fn downloads_in_pieces() {
        let body = data(100_000);
        let dir = tempfile::tempdir().unwrap();
        for mode in [Mode::Param, Mode::Partial, Mode::Whole] {
            let s = serve(body.clone(), mode).await;
            let f = file(&dir, body.len());
            finish(fill(&s.base, &f, 16_384)).await.unwrap();
            assert!(f.is_complete());
            assert_eq!(std::fs::read(f.path()).unwrap(), *body);
        }
        let s = serve(body.clone(), Mode::Partial).await;
        let f = file(&dir, body.len());
        f.write_at(20_000, &body[20_000..30_000]).unwrap();
        finish(fill(&s.base, &f, 16_384)).await.unwrap();
        assert_eq!(std::fs::read(f.path()).unwrap(), *body);
        assert_eq!(
            *s.ranges.lock().unwrap(),
            [(0, 16_383), (16_384, 19_999), (30_000, 46_383)]
                .into_iter()
                .chain([(46_384, 62_767), (62_768, 79_151), (79_152, 95_535)])
                .chain([(95_536, 99_999)])
                .collect::<Vec<_>>(),
            "skips what is there"
        );
    }

    #[tokio::test]
    async fn follows_a_seek() {
        let body = data(200_000);
        let s = serve(body.clone(), Mode::Slow(Duration::from_millis(2))).await;
        let dir = tempfile::tempdir().unwrap();
        let f = file(&dir, body.len());
        let job = fill(&s.base, &f, 65_536);
        while f.filled_bytes() < 8192 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // A blocking reader seeks far ahead while the first piece is still arriving.
        let reader = f.clone();
        let got = tokio::task::spawn_blocking(move || {
            let mut r = reader.reader();
            r.seek(SeekFrom::Start(150_000)).unwrap();
            let mut buf = vec![0; 4096];
            r.read_exact(&mut buf).unwrap();
            buf
        })
        .await
        .unwrap();
        assert_eq!(got, body[150_000..154_096]);
        finish(job).await.unwrap();
        assert_eq!(std::fs::read(f.path()).unwrap(), *body);

        let ranges = s.ranges.lock().unwrap().clone();
        assert_eq!(ranges[0], (0, 65_535));
        assert_eq!(
            ranges[1],
            (150_000, 199_999),
            "jumps to the seek: {ranges:?}"
        );
        assert!(
            ranges[2..].iter().any(|r| r.0 > 8192 && r.0 < 65_536),
            "the cut-short piece is completed later: {ranges:?}"
        );
    }

    #[tokio::test]
    async fn expired_url_waits_for_a_fresh_one() {
        let body = data(50_000);
        let s = serve(body.clone(), Mode::Partial).await;
        let dir = tempfile::tempdir().unwrap();
        let f = file(&dir, body.len());
        let (tx, mut expired) = tokio::sync::mpsc::unbounded_channel();
        let client = reqwest::Client::new();
        let on_expired = move || {
            let _ = tx.send(());
        };
        let job = StreamFill::with_piece(client, s.expired.clone(), f.clone(), on_expired, 16_384);
        expired.recv().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!job.done(), "waits instead of failing");
        assert_eq!(f.filled_bytes(), 0);
        job.retarget(s.base.clone());
        finish(job).await.unwrap();
        assert_eq!(std::fs::read(f.path()).unwrap(), *body);
    }

    #[tokio::test]
    async fn cancel_stops_a_hanging_request() {
        let s = serve(data(100_000), Mode::Stall(5000)).await;
        let dir = tempfile::tempdir().unwrap();
        let f = file(&dir, 100_000);
        let job = fill(&s.base, &f, 65_536);
        while f.filled_bytes() < 5000 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(!job.done());
        job.cancel();
        assert!(matches!(finish(job).await, Err(FetchError::Cancelled)));
        assert_eq!((f.filled_bytes(), f.available(0)), (5000, 5000));
    }

    #[tokio::test]
    async fn http_error_fails_the_readers() {
        let body = data(1000);
        let s = serve(body, Mode::Partial).await;
        let dir = tempfile::tempdir().unwrap();
        // The file claims more bytes than the server has: the request past its end gets 416.
        let f = file(&dir, 2000);
        let err = finish(fill(&s.base, &f, 1000)).await.unwrap_err();
        assert!(matches!(err, FetchError::Http(416)), "{err:?}");
        let mut r = f.reader();
        r.seek(SeekFrom::Start(1500)).unwrap();
        let e = r.read(&mut [0; 10]).unwrap_err();
        assert_eq!(e.to_string(), err.to_string());
    }
}
