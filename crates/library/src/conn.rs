//! Connection handling: one writer thread owns the write connection, and a small pool
//! of read-only connections serves queries. Calls block; async callers wrap them in
//! `spawn_blocking`.

use crate::{Error, Result, db};
use rusqlite::{Connection, Transaction};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

type WriteJob = Box<dyn FnOnce(&mut Connection) + Send>;

const READERS: usize = 3;

#[derive(Clone)]
pub struct Db {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    writer: mpsc::Sender<WriteJob>,
    readers: Mutex<Vec<Connection>>,
    reader_free: Condvar,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let writer_conn = db::open_writer(path)?;
        let readers = (0..READERS).map(|_| db::open_reader(path)).collect::<Result<Vec<_>>>()?;
        let (tx, rx) = mpsc::channel::<WriteJob>();
        std::thread::Builder::new().name("db-writer".into()).spawn(move || {
            let mut conn = writer_conn;
            while let Ok(job) = rx.recv() {
                // A panicking job must not take the writer down with it: its transaction is
                // rolled back on unwind, its caller gets an error, the next jobs still run.
                let run = std::panic::AssertUnwindSafe(|| job(&mut conn));
                if std::panic::catch_unwind(run).is_err() {
                    tracing::error!("a database write panicked; rolled back");
                }
            }
        })?;
        Ok(Self {
            inner: Arc::new(Inner {
                path: path.to_owned(),
                writer: tx,
                readers: Mutex::new(readers),
                reader_free: Condvar::new(),
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Runs `f` inside a transaction on the writer thread and waits for the result.
    /// An error from `f` rolls the transaction back.
    pub fn write<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<R> + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(1);
        let job: WriteJob = Box::new(move |conn| {
            let res = conn.transaction().map_err(Error::from).and_then(|txn| {
                let r = f(&txn)?;
                txn.commit()?;
                Ok(r)
            });
            let _ = tx.send(res);
        });
        self.inner.writer.send(job).map_err(|_| Error::WorkerGone)?;
        rx.recv().map_err(|_| Error::WorkerGone)?
    }

    /// Runs `f` on a pooled read-only connection.
    pub fn read<R, F>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> Result<R>,
    {
        let conn = {
            let mut pool = self.inner.readers.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(c) = pool.pop() {
                    break c;
                }
                pool = self.inner.reader_free.wait(pool).unwrap_or_else(|e| e.into_inner());
            }
        };
        // Returned to the pool on drop, so a panicking `f` does not shrink it for good.
        let lease = Lease { inner: &self.inner, conn: Some(conn) };
        f(lease.conn.as_ref().expect("leased connection"))
    }
}

struct Lease<'a> {
    inner: &'a Inner,
    conn: Option<Connection>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            self.inner.readers.lock().unwrap_or_else(|e| e.into_inner()).push(c);
            self.inner.reader_free.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings rows besides `device_id`, which every database has.
    fn settings_count(db: &Db) -> i64 {
        db.read(|c| Ok(c.query_row("SELECT count(*) FROM settings WHERE key <> 'device_id'", [], |r| r.get(0))?))
            .unwrap()
    }

    #[test]
    fn panics_do_not_drain_the_pool_or_stop_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        for _ in 0..READERS + 1 {
            let d = db.clone();
            let r = std::thread::spawn(move || d.read(|_| -> Result<()> { panic!("reader") })).join();
            assert!(r.is_err());
        }
        let r = db.write(|t| -> Result<()> {
            t.execute("INSERT INTO settings(key, value) VALUES ('k', '1')", [])?;
            panic!("writer")
        });
        assert!(r.is_err());
        db.write(|t| {
            t.execute("INSERT INTO settings(key, value) VALUES ('j', '1')", [])?;
            Ok(())
        })
        .unwrap();
        // The panicked write was rolled back; reads still find a free connection.
        assert_eq!(settings_count(&db), 1);
    }

    #[test]
    fn write_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        db.write(|t| {
            t.execute("INSERT INTO settings(key, value) VALUES ('k', '1')", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(settings_count(&db), 1);
    }

    #[test]
    fn failed_write_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let res: Result<()> = db.write(|t| {
            t.execute("INSERT INTO settings(key, value) VALUES ('k', '1')", [])?;
            Err(Error::Other("boom".into()))
        });
        assert!(res.is_err());
        assert_eq!(settings_count(&db), 0);
    }
}
