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
                job(&mut conn);
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
            let mut pool = self.inner.readers.lock().expect("reader pool poisoned");
            loop {
                if let Some(c) = pool.pop() {
                    break c;
                }
                pool = self.inner.reader_free.wait(pool).expect("reader pool poisoned");
            }
        };
        let res = f(&conn);
        self.inner.readers.lock().expect("reader pool poisoned").push(conn);
        self.inner.reader_free.notify_one();
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_count(db: &Db) -> i64 {
        db.read(|c| Ok(c.query_row("SELECT count(*) FROM settings", [], |r| r.get(0))?)).unwrap()
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
