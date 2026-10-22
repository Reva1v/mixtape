//! The two sides of a session: the PC [`serve`]s, the phone [`pair`]s, [`exchange`]s and
//! [`unpair`]s.
//!
//! An exchange: client → `Cursors`; server → `Events`… `Done {cursors}`; client → `Events`…
//! `Done`; server → `Ack`. Each received batch is merged in its own transaction, so a cut
//! connection keeps what arrived, and the next exchange starts from the new cursors.

use super::wire::{Msg, Refusal, Session, Stream, read_hello, refuse};
use super::{PairCode, PeerError, now_s};
use mixtape_library::Db;
use mixtape_library::peer_store::{self, BATCH, Cursors, Pairing};
use mixtape_library::repo::now_ms;
use mixtape_library::settings;

/// What an exchange moved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// New plays stored here.
    pub received: usize,
    /// Plays sent to the other side.
    pub sent: usize,
}

/// How a served connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    /// A phone paired with the shown code; the code is spent.
    Paired(Pairing),
    Exchanged(Report),
    /// The phone unpaired; the pairing is removed here too.
    Unpaired,
}

/// Answers one connection on the PC. `code` is the pairing code on screen, if any; a
/// pairing replaces the stored one.
pub async fn serve<S: Stream>(mut stream: S, db: &Db, code: Option<&PairCode>) -> Result<Served, PeerError> {
    let hello = read_hello(&mut stream).await?;
    let (me, my_name) = blocking(db, |c| Ok((settings::device_id(c)?, settings::device_name(c)?))).await?;
    if hello.pair {
        let Some(code) = code else {
            refuse(&mut stream, Refusal::NoCode).await?;
            return Err(PeerError::Refused(Refusal::NoCode));
        };
        if code.is_expired(now_s()) {
            refuse(&mut stream, Refusal::Expired).await?;
            return Err(PeerError::Expired);
        }
        let mut s = Session::server(stream, &hello, &me, &code.key).await?;
        let Msg::Pair { device, name } = s.recv().await? else {
            return Err(PeerError::Protocol("expected Pair".into()));
        };
        if device != s.peer_device {
            return Err(PeerError::Protocol("Pair from another device than Hello".into()));
        }
        let pairing = Pairing {
            peer_device: device,
            peer_name: name,
            key: code.key,
            addresses: Vec::new(),
            last_address: None,
            paired_at: now_ms(),
            last_exchange: None,
        };
        let p = pairing.clone();
        db_write(db, move |t| peer_store::save_pairing(t, &p)).await?;
        s.send(&Msg::Paired { device: me, name: my_name }).await?;
        return Ok(Served::Paired(pairing));
    }

    let pairing = blocking(db, peer_store::pairing).await?;
    let Some(pairing) = pairing.filter(|p| p.peer_device == hello.device) else {
        refuse(&mut stream, Refusal::UnknownDevice).await?;
        return Err(PeerError::NotPaired);
    };
    let mut s = Session::server(stream, &hello, &me, &pairing.key).await?;
    let theirs = match s.recv().await? {
        Msg::Cursors { cursors } => cursors,
        Msg::Unpair => {
            db_write(db, |t| peer_store::remove_pairing(t)).await?;
            s.send(&Msg::Ack).await?;
            return Ok(Served::Unpaired);
        }
        _ => return Err(PeerError::Protocol("expected Cursors".into())),
    };
    let mine = blocking(db, peer_store::cursors).await?;
    let sent = send_events(&mut s, db, theirs).await?;
    s.send(&Msg::Done { cursors: Some(mine) }).await?;
    let received = receive_events(&mut s, db).await?.0;
    s.send(&Msg::Ack).await?;
    db_write(db, |t| peer_store::note_exchange(t, None, now_ms())).await?;
    Ok(Served::Exchanged(Report { received, sent }))
}

/// Pairs this device (the phone) with the PC whose `code` was scanned and stores the
/// pairing, replacing any earlier one. `stream` is connected to one of the code's addresses.
pub async fn pair<S: Stream>(stream: S, db: &Db, code: &PairCode) -> Result<Pairing, PeerError> {
    code.check(now_s())?;
    let (me, my_name) = blocking(db, |c| Ok((settings::device_id(c)?, settings::device_name(c)?))).await?;
    let mut s = Session::client(stream, &me, &code.key, true).await?;
    s.send(&Msg::Pair { device: me, name: my_name }).await?;
    let Msg::Paired { device, name } = first_answer(&mut s).await? else {
        return Err(PeerError::Protocol("expected Paired".into()));
    };
    if device != s.peer_device {
        return Err(PeerError::Protocol("Paired from another device than Hello".into()));
    }
    let pairing = Pairing {
        peer_device: device,
        peer_name: if name.is_empty() { code.name.clone() } else { name },
        key: code.key,
        addresses: code.socket_addrs(),
        last_address: None,
        paired_at: now_ms(),
        last_exchange: None,
    };
    let p = pairing.clone();
    db_write(db, move |t| peer_store::save_pairing(t, &p)).await?;
    Ok(pairing)
}

/// Exchanges plays with the paired PC (client side). `address` is the one `stream` was
/// dialled to, remembered as the last good one.
pub async fn exchange<S: Stream>(stream: S, db: &Db, address: Option<&str>) -> Result<Report, PeerError> {
    let (me, pairing, mine) = blocking(db, |c| {
        Ok((settings::device_id(c)?, peer_store::pairing(c)?, peer_store::cursors(c)?))
    })
    .await?;
    let pairing = pairing.ok_or(PeerError::NotPaired)?;
    let mut s = Session::client(stream, &me, &pairing.key, false).await?;
    if s.peer_device != pairing.peer_device {
        return Err(PeerError::Protocol("another PC answered".into()));
    }
    s.send(&Msg::Cursors { cursors: mine }).await?;
    let (received, theirs) = match first_answer(&mut s).await? {
        Msg::Events { events } => {
            let n = merge(db, events).await?;
            let (more, theirs) = receive_events(&mut s, db).await?;
            (n + more, theirs)
        }
        Msg::Done { cursors } => (0, cursors),
        _ => return Err(PeerError::Protocol("expected Events".into())),
    };
    let theirs = theirs.ok_or_else(|| PeerError::Protocol("Done without cursors".into()))?;
    let sent = send_events(&mut s, db, theirs).await?;
    s.send(&Msg::Done { cursors: None }).await?;
    if s.recv().await? != Msg::Ack {
        return Err(PeerError::Protocol("expected Ack".into()));
    }
    let address = address.map(str::to_owned);
    db_write(db, move |t| peer_store::note_exchange(t, address.as_deref(), now_ms())).await?;
    Ok(Report { received, sent })
}

/// Tells the paired PC to forget this device, then forgets the pairing here. When the PC
/// can't be reached the caller removes the pairing itself
/// ([`peer_store::remove_pairing`]); the PC then refuses the next connection.
pub async fn unpair<S: Stream>(stream: S, db: &Db) -> Result<(), PeerError> {
    let (me, pairing) = blocking(db, |c| Ok((settings::device_id(c)?, peer_store::pairing(c)?))).await?;
    let pairing = pairing.ok_or(PeerError::NotPaired)?;
    let mut s = Session::client(stream, &me, &pairing.key, false).await?;
    s.send(&Msg::Unpair).await?;
    if first_answer(&mut s).await? != Msg::Ack {
        return Err(PeerError::Protocol("expected Ack".into()));
    }
    db_write(db, |t| peer_store::remove_pairing(t)).await?;
    Ok(())
}

/// The server's first encrypted frame. It hangs up without one when our first frame didn't
/// open with its key: that is a key mismatch, not a network fault.
async fn first_answer<S: Stream>(s: &mut Session<S>) -> Result<Msg, PeerError> {
    match s.recv().await {
        Err(PeerError::Closed) => Err(PeerError::BadKey),
        r => r,
    }
}

/// Sends what `theirs` lacks in batches; returns how many.
async fn send_events<S: Stream>(s: &mut Session<S>, db: &Db, mut theirs: Cursors) -> Result<usize, PeerError> {
    let mut sent = 0;
    loop {
        let from = theirs.clone();
        let events = blocking(db, move |c| peer_store::events_after(c, &from, BATCH)).await?;
        if events.is_empty() {
            return Ok(sent);
        }
        peer_store::advance(&mut theirs, &events);
        sent += events.len();
        s.send(&Msg::Events { events }).await?;
    }
}

/// Merges `Events` until `Done`; returns the new plays and the cursors `Done` carried.
async fn receive_events<S: Stream>(s: &mut Session<S>, db: &Db) -> Result<(usize, Option<Cursors>), PeerError> {
    let mut n = 0;
    loop {
        match s.recv().await? {
            Msg::Events { events } => n += merge(db, events).await?,
            Msg::Done { cursors } => return Ok((n, cursors)),
            _ => return Err(PeerError::Protocol("expected Events or Done".into())),
        }
    }
}

async fn merge(db: &Db, events: Vec<mixtape_library::peer_store::PeerEvent>) -> Result<usize, PeerError> {
    if events.len() > BATCH {
        return Err(PeerError::Protocol("batch too large".into()));
    }
    db_write(db, move |t| peer_store::merge_events(t, &events)).await
}

/// Runs a read off the async threads.
async fn blocking<R, F>(db: &Db, f: F) -> Result<R, PeerError>
where
    R: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> mixtape_library::Result<R> + Send + 'static,
{
    let db = db.clone();
    tokio::task::spawn_blocking(move || db.read(f))
        .await
        .map_err(|e| PeerError::Io(std::io::Error::other(e.to_string())))?
        .map_err(Into::into)
}

/// Runs a write transaction off the async threads.
async fn db_write<R, F>(db: &Db, f: F) -> Result<R, PeerError>
where
    R: Send + 'static,
    F: FnOnce(&rusqlite::Transaction<'_>) -> mixtape_library::Result<R> + Send + 'static,
{
    let db = db.clone();
    tokio::task::spawn_blocking(move || db.write(f))
        .await
        .map_err(|e| PeerError::Io(std::io::Error::other(e.to_string())))?
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixtape_core::{AlbumId, AlbumRef, ArtistId, ArtistRef, TrackId};
    use mixtape_library::repo::{TrackMeta, upsert_track};
    use mixtape_library::stats;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    fn open() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        (dir, db)
    }

    /// `n` plays of tracks `prefix0..prefix9`.
    fn seed(db: &Db, prefix: &str, n: usize) {
        let prefix = prefix.to_owned();
        db.write(move |t| {
            for i in 0..10 {
                upsert_track(
                    t,
                    &TrackMeta {
                        id: TrackId::from(format!("{prefix}{i}").as_str()),
                        title: format!("{prefix} song {i}"),
                        artists: vec![ArtistRef { id: Some(ArtistId::from("UCa")), name: "Artist".into() }],
                        album: Some(AlbumRef { id: AlbumId::from("MPREa"), title: "Album".into() }),
                        track_no: None,
                        duration_ms: 200_000,
                        cover_key: None,
                    },
                )?;
            }
            for i in 0..n {
                stats::record(t, &TrackId::from(format!("{prefix}{}", i % 10).as_str()), i as i64 * 1000, 60_000)?;
            }
            Ok(())
        })
        .unwrap();
    }

    fn plays(db: &Db) -> i64 {
        db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM play_events", [], |r| r.get(0))?)).unwrap()
    }

    fn origins(db: &Db) -> Vec<(String, i64)> {
        db.read(|c| {
            let mut stmt = c.prepare("SELECT device, seq FROM play_events ORDER BY device, seq")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap()
    }

    /// Pairs `phone` with `pc` over an in-memory pipe.
    async fn paired(pc: &Db, phone: &Db) {
        let code = PairCode::new("PC", super::super::PORT, vec!["127.0.0.1".into()], now_s()).unwrap();
        let (a, b) = tokio::io::duplex(1 << 16);
        let (served, paired) = tokio::join!(serve(b, pc, Some(&code)), pair(a, phone, &code));
        let Served::Paired(on_pc) = served.unwrap() else { panic!("not paired") };
        let on_phone = paired.unwrap();
        assert_eq!(on_pc.key, on_phone.key);
        assert_eq!(on_phone.addresses, ["127.0.0.1:47821"]);
        assert_eq!(on_pc.peer_device, phone.read(settings::device_id).unwrap());
        assert_eq!(on_phone.peer_device, pc.read(settings::device_id).unwrap());
    }

    async fn sync(pc: &Db, phone: &Db) -> Result<(Report, Served), PeerError> {
        let (a, b) = tokio::io::duplex(1 << 16);
        let (served, report) = tokio::join!(serve(b, pc, None), exchange(a, phone, Some("127.0.0.1:47821")));
        Ok((report?, served?))
    }

    #[tokio::test]
    async fn exchange_both_ways_then_nothing() {
        let (_a, pc) = open();
        let (_b, phone) = open();
        paired(&pc, &phone).await;
        seed(&pc, "p", 2);
        seed(&phone, "m", 3);
        let (report, served) = sync(&pc, &phone).await.unwrap();
        assert_eq!(report, Report { received: 2, sent: 3 });
        assert_eq!(served, Served::Exchanged(Report { received: 3, sent: 2 }));
        assert_eq!((plays(&pc), plays(&phone)), (5, 5));
        assert_eq!(origins(&pc), origins(&phone));
        // The phone got stubs of the PC's tracks, with names.
        let t = phone.read(|c| mixtape_library::repo::track(c, &TrackId::from("p0"))).unwrap().unwrap();
        assert_eq!((t.title.as_str(), t.artists[0].name.as_str()), ("p song 0", "Artist"));
        let p = phone.read(peer_store::pairing).unwrap().unwrap();
        assert_eq!(p.last_address.as_deref(), Some("127.0.0.1:47821"));
        assert!(p.last_exchange.is_some());
        assert!(pc.read(peer_store::pairing).unwrap().unwrap().last_exchange.is_some());

        let (report, served) = sync(&pc, &phone).await.unwrap();
        assert_eq!(report, Report::default());
        assert_eq!(served, Served::Exchanged(Report::default()));
        assert_eq!((plays(&pc), plays(&phone)), (5, 5));
        // Totals agree on both sides.
        let total = |db: &Db| db.read(|c| stats::listening(c, i64::MIN, 0)).unwrap().listened_ms;
        assert_eq!(total(&pc), total(&phone));
    }

    /// Fails writes after `left` bytes, like a dropped Wi-Fi.
    struct Cut<S> {
        inner: S,
        left: usize,
    }

    impl<S: AsyncRead + Unpin> AsyncRead for Cut<S> {
        fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl<S: AsyncWrite + Unpin> AsyncWrite for Cut<S> {
        fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
            if self.left == 0 {
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }
            let n = buf.len().min(self.left);
            let r = Pin::new(&mut self.inner).poll_write(cx, &buf[..n]);
            if let Poll::Ready(Ok(w)) = r {
                self.left -= w;
            }
            r
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn a_cut_exchange_resumes_without_duplicates() {
        let (_a, pc) = open();
        let (_b, phone) = open();
        paired(&pc, &phone).await;
        seed(&pc, "p", 2_500);
        seed(&phone, "m", 40);
        // The PC's line drops in the middle of its second batch.
        let (a, b) = tokio::io::duplex(1 << 16);
        let one_batch = 1000 * 200;
        let cut = Cut { inner: b, left: one_batch * 3 / 2 };
        let (served, report) = tokio::join!(serve(cut, &pc, None), exchange(a, &phone, None));
        assert!(served.is_err() && report.is_err());
        let got = plays(&phone);
        assert!((1040..2540).contains(&got), "the first batch stayed: {got}");
        assert_eq!(plays(&pc), 2_500);

        let (report, _) = sync(&pc, &phone).await.unwrap();
        assert_eq!(report, Report { received: (2540 - got) as usize, sent: 40 });
        assert_eq!((plays(&pc), plays(&phone)), (2_540, 2_540));
        assert_eq!(origins(&pc), origins(&phone));
        let (report, _) = sync(&pc, &phone).await.unwrap();
        assert_eq!(report, Report::default());
    }

    #[tokio::test]
    async fn a_wrong_key_gets_nothing() {
        let (_a, pc) = open();
        let (_b, phone) = open();
        paired(&pc, &phone).await;
        seed(&pc, "p", 5);
        seed(&phone, "m", 5);
        // Someone with the right device id but another key.
        phone
            .write(|t| {
                let mut p = peer_store::pairing(t)?.unwrap();
                p.key[0] ^= 1;
                peer_store::save_pairing(t, &p)
            })
            .unwrap();
        let err = sync(&pc, &phone).await.unwrap_err();
        assert!(err.needs_pairing(), "{err:?}");
        let (a, b) = tokio::io::duplex(1 << 16);
        let (served, report) = tokio::join!(serve(b, &pc, None), exchange(a, &phone, None));
        assert!(matches!(served, Err(PeerError::BadKey)), "{served:?}");
        assert!(matches!(report, Err(PeerError::BadKey)), "{report:?}");
        assert_eq!((plays(&pc), plays(&phone)), (5, 5));

        // An unknown device is refused before any key is used.
        let (_c, stranger) = open();
        let p = phone.read(peer_store::pairing).unwrap().unwrap();
        stranger.write(move |t| peer_store::save_pairing(t, &p)).unwrap();
        let (a, b) = tokio::io::duplex(1 << 16);
        let (served, report) = tokio::join!(serve(b, &pc, None), exchange(a, &stranger, None));
        assert!(matches!(served, Err(PeerError::NotPaired)));
        assert!(matches!(report, Err(PeerError::Refused(Refusal::UnknownDevice))), "{report:?}");
    }

    #[tokio::test]
    async fn expired_or_missing_codes_dont_pair() {
        let (_a, pc) = open();
        let (_b, phone) = open();
        let mut code = PairCode::new("PC", 47821, Vec::new(), now_s()).unwrap();
        // The PC is not showing a code.
        let (a, b) = tokio::io::duplex(1 << 16);
        let (served, paired) = tokio::join!(serve(b, &pc, None), pair(a, &phone, &code));
        assert!(served.is_err());
        assert!(matches!(paired, Err(PeerError::Refused(Refusal::NoCode))), "{paired:?}");
        // The phone checks the time before dialling.
        code.expires_at = now_s() - 1;
        let (a, _b) = tokio::io::duplex(1 << 16);
        assert!(matches!(pair(a, &phone, &code).await, Err(PeerError::Expired)));
        // And the PC checks it too.
        let (a, b) = tokio::io::duplex(1 << 16);
        let live = PairCode { expires_at: now_s() + 60, ..code.clone() };
        let (served, paired) = tokio::join!(serve(b, &pc, Some(&code)), pair(a, &phone, &live));
        assert!(matches!(served, Err(PeerError::Expired)));
        assert!(matches!(paired, Err(PeerError::Refused(Refusal::Expired))), "{paired:?}");
        assert_eq!((pc.read(peer_store::pairing).unwrap(), phone.read(peer_store::pairing).unwrap()), (None, None));
    }

    #[tokio::test]
    async fn unpair_removes_both_sides() {
        let (_a, pc) = open();
        let (_b, phone) = open();
        paired(&pc, &phone).await;
        let (a, b) = tokio::io::duplex(1 << 16);
        let (served, done) = tokio::join!(serve(b, &pc, None), unpair(a, &phone));
        done.unwrap();
        assert_eq!(served.unwrap(), Served::Unpaired);
        assert_eq!((pc.read(peer_store::pairing).unwrap(), phone.read(peer_store::pairing).unwrap()), (None, None));
    }
}
