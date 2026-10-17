//! A file downloaded in pieces while it plays: preallocated to its full length, with the map
//! of byte ranges already written under a `Mutex` + `Condvar`.
//!
//! The downloader writes with [`SparseCacheFile::write_at`] and follows
//! [`SparseCacheFile::wanted`], the byte the reader needs next, so a seek retargets it. Readers
//! ([`SparseReader`]) block until their bytes arrive, or, non-blocking, answer `WouldBlock` so
//! the engine thread never waits inside symphonia. Once complete the file is moved into the
//! cache or library with [`SparseCacheFile::finalize`].

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use symphonia::core::io::MediaSource;

/// How long a blocking read waits for its bytes by default.
pub const READ_TIMEOUT: Duration = Duration::from_secs(20);

pub struct SparseCacheFile {
    file: File,
    len: u64,
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    /// Written byte ranges: sorted, disjoint and not touching.
    filled: Vec<Range<u64>>,
    wanted: u64,
    failed: Option<String>,
    cancelled: bool,
    path: PathBuf,
}

impl SparseCacheFile {
    /// Creates (or truncates) `path` at `len` bytes, none of them downloaded yet.
    pub fn create(path: impl Into<PathBuf>, len: u64) -> io::Result<Arc<Self>> {
        let path = path.into();
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.set_len(len)?;
        Ok(Arc::new(Self {
            file,
            len,
            state: Mutex::new(State {
                filled: Vec::new(),
                wanted: 0,
                failed: None,
                cancelled: false,
                path,
            }),
            changed: Condvar::new(),
        }))
    }

    /// Full size of the file.
    pub fn total_len(&self) -> u64 {
        self.len
    }

    /// Where the file is now (it moves on [`SparseCacheFile::finalize`]).
    pub fn path(&self) -> PathBuf {
        self.lock().path.clone()
    }

    /// Stores downloaded bytes and wakes the readers waiting for them.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let end = offset + data.len() as u64;
        if end > self.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("write past the end: {end} > {}", self.len),
            ));
        }
        if data.is_empty() {
            return Ok(());
        }
        write_all_at(&self.file, data, offset)?;
        insert(&mut self.lock().filled, offset..end);
        self.changed.notify_all();
        Ok(())
    }

    /// The download gave up: readers waiting for missing bytes get `message` as an error.
    pub fn fail(&self, message: impl Into<String>) {
        self.lock().failed = Some(message.into());
        self.changed.notify_all();
    }

    /// Nobody needs the rest: waiting readers return an error, the downloader stops.
    pub fn cancel(&self) {
        self.lock().cancelled = true;
        self.changed.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }

    pub fn is_complete(&self) -> bool {
        let st = self.lock();
        self.len == 0 || st.filled.first() == Some(&(0..self.len))
    }

    /// Downloaded byte ranges, in order.
    pub fn filled(&self) -> Vec<Range<u64>> {
        self.lock().filled.clone()
    }

    pub fn filled_bytes(&self) -> u64 {
        self.lock().filled.iter().map(|r| r.end - r.start).sum()
    }

    /// Downloaded bytes from `offset` on without a hole.
    pub fn available(&self, offset: u64) -> u64 {
        available(&self.lock().filled, offset)
    }

    /// Whether `offset..offset + ahead` (cut at the end of the file) is downloaded, or nothing
    /// more will arrive (failed, cancelled) and a read would return at once.
    pub fn ready_at(&self, offset: u64, ahead: u64) -> bool {
        let st = self.lock();
        let need = ahead.min(self.len.saturating_sub(offset));
        st.cancelled || st.failed.is_some() || available(&st.filled, offset) >= need
    }

    /// [`SparseCacheFile::ready_at`] the byte the reader needs next.
    pub fn ready(&self, ahead: u64) -> bool {
        let wanted = self.wanted();
        self.ready_at(wanted, ahead)
    }

    /// The byte the reader needs next: its position, moved by every read and seek. The
    /// downloader continues from here.
    pub fn wanted(&self) -> u64 {
        self.lock().wanted
    }

    /// Points the downloader at `offset`, as a reader seeking there would.
    pub fn want(&self, offset: u64) {
        self.lock().wanted = offset;
    }

    /// The first missing range at or after `from`; when everything after it is there, the first
    /// missing range of the file. `None` once complete.
    pub fn next_gap(&self, from: u64) -> Option<Range<u64>> {
        let st = self.lock();
        gap_from(&st.filled, self.len, from).or_else(|| gap_from(&st.filled, self.len, 0))
    }

    /// A blocking reader at the start of the file (moves [`SparseCacheFile::wanted`] there).
    pub fn reader(self: &Arc<Self>) -> SparseReader {
        self.want(0);
        SparseReader {
            file: self.clone(),
            pos: 0,
            timeout: READ_TIMEOUT,
            nonblocking: false,
        }
    }

    /// Moves the complete file to `dest` (creating folders, replacing a file there): a rename,
    /// or a copy when `dest` is on another volume. Open readers keep working.
    pub fn finalize(&self, dest: &Path) -> io::Result<()> {
        if !self.is_complete() {
            return Err(io::Error::other("the file is not completely downloaded"));
        }
        self.file.sync_data()?;
        if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let from = self.path();
        if std::fs::rename(&from, dest).is_err() {
            std::fs::copy(&from, dest)?;
            std::fs::remove_file(&from)?;
        }
        self.lock().path = dest.to_path_buf();
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Reads a [`SparseCacheFile`] as a seekable stream of its full length.
pub struct SparseReader {
    file: Arc<SparseCacheFile>,
    pos: u64,
    timeout: Duration,
    nonblocking: bool,
}

impl SparseReader {
    /// How long a blocking read waits for missing bytes before failing with `TimedOut`.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Non-blocking, a read of missing bytes fails with `WouldBlock` at once.
    pub fn set_nonblocking(&mut self, nonblocking: bool) {
        self.nonblocking = nonblocking;
    }

    pub fn position(&self) -> u64 {
        self.pos
    }
}

impl Read for SparseReader {
    /// Returns what is downloaded from the position on (at most `buf.len()`), waiting until at
    /// least one byte is there. 0 at or past the end.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let f = &*self.file;
        if buf.is_empty() || self.pos >= f.len {
            return Ok(0);
        }
        let deadline = Instant::now() + self.timeout;
        let mut st = f.lock();
        st.wanted = self.pos;
        let n = loop {
            let n = available(&st.filled, self.pos);
            if n > 0 {
                break n.min(buf.len() as u64) as usize;
            }
            if st.cancelled {
                return Err(io::Error::other("the download was cancelled"));
            }
            if let Some(msg) = &st.failed {
                return Err(io::Error::other(msg.clone()));
            }
            if self.nonblocking {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the download is too slow",
                ));
            }
            st = f
                .changed
                .wait_timeout(st, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        st.wanted = self.pos + n as u64;
        drop(st);
        // Written ranges never shrink, so the bytes stay there without the lock.
        let n = read_at(&f.file, &mut buf[..n], self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SparseReader {
    /// Any position is allowed; past the end reads return 0.
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(d) => self.file.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        let pos = u64::try_from(pos)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        self.pos = pos;
        self.file.want(pos);
        Ok(pos)
    }
}

impl MediaSource for SparseReader {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.file.len)
    }
}

/// Adds `r` to the sorted set, merging what it overlaps or touches.
fn insert(filled: &mut Vec<Range<u64>>, r: Range<u64>) {
    let i = filled.partition_point(|f| f.end < r.start);
    let (mut start, mut end) = (r.start, r.end);
    let mut j = i;
    while j < filled.len() && filled[j].start <= end {
        start = start.min(filled[j].start);
        end = end.max(filled[j].end);
        j += 1;
    }
    filled.splice(i..j, std::iter::once(start..end));
}

fn available(filled: &[Range<u64>], offset: u64) -> u64 {
    let i = filled.partition_point(|f| f.end <= offset);
    match filled.get(i) {
        Some(f) if f.start <= offset => f.end - offset,
        _ => 0,
    }
}

fn gap_from(filled: &[Range<u64>], len: u64, from: u64) -> Option<Range<u64>> {
    let i = filled.partition_point(|f| f.end <= from);
    let (start, next) = match filled.get(i) {
        Some(f) if f.start <= from => (f.end, filled.get(i + 1)),
        other => (from, other),
    };
    (start < len).then(|| start..next.map_or(len, |f| f.start))
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, offset)
}

#[cfg(windows)]
fn write_at(f: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(f, buf, offset)
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, offset)
}

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(f, buf, offset)
}

fn write_all_at(f: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match write_at(f, buf, offset)? {
            0 => return Err(io::ErrorKind::WriteZero.into()),
            n => {
                buf = &buf[n..];
                offset += n as u64;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // lists of filled ranges
mod tests {
    use super::*;
    use std::thread;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn new(len: usize) -> (tempfile::TempDir, Arc<SparseCacheFile>) {
        let dir = tempfile::tempdir().unwrap();
        let f = SparseCacheFile::create(dir.path().join("x/a.part"), len as u64).unwrap();
        (dir, f)
    }

    /// Writes `body[range]` after `delay` from another thread.
    fn later(
        f: &Arc<SparseCacheFile>,
        body: &[u8],
        range: Range<usize>,
        delay: u64,
    ) -> thread::JoinHandle<()> {
        let (f, piece, at) = (f.clone(), body[range.clone()].to_vec(), range.start);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay));
            f.write_at(at as u64, &piece).unwrap();
        })
    }

    #[test]
    fn intervals_merge_and_report_gaps() {
        let (_d, f) = new(100);
        assert_eq!(f.next_gap(0), Some(0..100));
        f.write_at(10, &[1; 10]).unwrap();
        f.write_at(30, &[1; 10]).unwrap();
        assert_eq!(f.filled(), [10..20, 30..40]);
        f.write_at(20, &[1; 10]).unwrap();
        assert_eq!(f.filled(), [10..40]);
        f.write_at(5, &[1; 40]).unwrap();
        assert_eq!((f.filled(), f.filled_bytes()), (vec![5..45], 40));
        assert_eq!(
            (f.available(10), f.available(45), f.available(0)),
            (35, 0, 0)
        );
        assert_eq!(f.next_gap(0), Some(0..5));
        assert_eq!(f.next_gap(7), Some(45..100));
        f.write_at(60, &[1; 40]).unwrap();
        assert_eq!(f.next_gap(70), Some(0..5), "wraps to the first hole");
        assert!(f.ready_at(60, 1000) && !f.ready_at(40, 10) && f.ready_at(40, 5));
        assert!(f.write_at(95, &[1; 10]).is_err());
        f.write_at(0, &[1; 100]).unwrap();
        assert!(f.is_complete());
        assert_eq!(f.next_gap(0), None);
    }

    #[test]
    fn read_waits_for_the_bytes() {
        let body = data(10_000);
        let (_d, f) = new(body.len());
        f.write_at(0, &body[..4000]).unwrap();
        let mut r = f.reader();
        let mut out = vec![0; 6000];
        assert_eq!(r.read(&mut out).unwrap(), 4000, "what is there, at once");
        assert_eq!(&out[..4000], &body[..4000]);
        assert_eq!(f.wanted(), 4000);

        let w = later(&f, &body, 4000..10_000, 50);
        let mut rest = Vec::new();
        r.read_to_end(&mut rest).unwrap();
        w.join().unwrap();
        assert_eq!(rest, body[4000..]);
        assert_eq!(r.read(&mut out).unwrap(), 0, "at the end");
    }

    #[test]
    fn seek_beyond_the_filled_region() {
        let body = data(10_000);
        let (_d, f) = new(body.len());
        f.write_at(0, &body[..1000]).unwrap();
        let mut r = f.reader();
        assert_eq!(r.seek(SeekFrom::Start(7000)).unwrap(), 7000);
        assert_eq!(f.wanted(), 7000, "the downloader is retargeted");
        assert_eq!(f.next_gap(f.wanted()), Some(7000..10_000));
        let w = later(&f, &body, 7000..8000, 50);
        let mut out = [0; 500];
        r.read_exact(&mut out).unwrap();
        w.join().unwrap();
        assert_eq!(out, body[7000..7500]);
        assert_eq!(r.seek(SeekFrom::Current(-6500)).unwrap(), 1000);
        assert_eq!(f.next_gap(f.wanted()), Some(1000..7000));
    }

    #[test]
    fn seek_past_the_end_reads_nothing() {
        let (_d, f) = new(100);
        let mut r = f.reader();
        r.set_timeout(Duration::from_millis(10));
        assert_eq!(r.seek(SeekFrom::End(50)).unwrap(), 150);
        assert_eq!(r.read(&mut [0; 10]).unwrap(), 0);
        assert!(r.seek(SeekFrom::End(-200)).is_err());
        assert_eq!((r.is_seekable(), r.byte_len()), (true, Some(100)));
    }

    #[test]
    fn read_times_out_or_would_block() {
        let (_d, f) = new(100);
        let mut r = f.reader();
        r.set_timeout(Duration::from_millis(50));
        let t = Instant::now();
        let err = r.read(&mut [0; 10]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(t.elapsed() >= Duration::from_millis(50));

        r.set_nonblocking(true);
        let t = Instant::now();
        let err = r.read(&mut [0; 10]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert!(t.elapsed() < Duration::from_millis(20));
        f.write_at(0, &[7; 4]).unwrap();
        assert_eq!(r.read(&mut [0; 10]).unwrap(), 4, "short read, no waiting");
    }

    #[test]
    fn fail_and_cancel_wake_the_reader() {
        for cancel in [false, true] {
            let (_d, f) = new(100);
            f.write_at(0, &[1; 10]).unwrap();
            let mut r = f.reader();
            r.read_exact(&mut [0; 10]).unwrap();
            let g = f.clone();
            let t = thread::spawn(move || {
                thread::sleep(Duration::from_millis(50));
                if cancel { g.cancel() } else { g.fail("gone") }
            });
            let started = Instant::now();
            let err = r.read(&mut [0; 10]).unwrap_err();
            t.join().unwrap();
            assert!(started.elapsed() < READ_TIMEOUT / 2);
            assert_eq!(err.kind(), io::ErrorKind::Other);
            assert!(cancel == f.is_cancelled());
            if !cancel {
                assert_eq!(err.to_string(), "gone");
            }
            // Bytes already there are still served.
            r.seek(SeekFrom::Start(0)).unwrap();
            assert_eq!(r.read(&mut [0; 20]).unwrap(), 10);
            assert!(f.ready(1000), "nothing to wait for any more");
        }
    }

    #[test]
    fn finalize_moves_the_complete_file() {
        let body = data(5000);
        let (dir, f) = new(body.len());
        let dest = dir.path().join("cache/b/a.webm");
        f.write_at(0, &body[..4000]).unwrap();
        assert!(f.finalize(&dest).is_err(), "incomplete");
        f.write_at(4000, &body[4000..]).unwrap();
        let mut r = f.reader();
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"old").unwrap();
        f.finalize(&dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!dir.path().join("x/a.part").exists());
        assert_eq!(f.path(), dest);
        let mut all = Vec::new();
        r.read_to_end(&mut all).unwrap();
        assert_eq!(all, body, "an open reader keeps working");
    }
}
