//! A stand-in PC for checking the phone's stats sync without the desktop app:
//! `cargo run -p mixtape-sync --example peer_probe -- <dir> [address...]`.
//! Opens (or creates) a library in `<dir>`, logs three plays on first run, prints a pairing
//! code for the given addresses (default `10.0.2.2`, the host as the Android emulator sees
//! it) and serves the peer protocol on port 47821, printing every result and the totals.
//! Touches nothing but `<dir>`.

use mixtape_core::{ArtistRef, TrackId};
use mixtape_library::{Db, repo, stats};
use mixtape_sync::peer::{self, PairCode, Served};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().ok_or("usage: peer_probe <dir> [address...]")?);
    let mut addresses: Vec<String> = args.collect();
    if addresses.is_empty() {
        addresses.push("10.0.2.2".into());
    }
    std::fs::create_dir_all(&dir)?;
    let db = Db::open(&dir.join("library.db"))?;
    mixtape_library::settings::set(&db, mixtape_library::settings::DEVICE_NAME, &"Probe PC".to_owned())?;
    if db.read(|c| stats::listening(c, i64::MIN, 0))?.plays == 0 {
        db.write(|t| {
            for (i, (id, title)) in [("dQw4w9WgXcQ", "Probe Song A"), ("9bZkp7q19f0", "Probe Song B")].into_iter().enumerate() {
                let id = TrackId::new(id);
                repo::upsert_track(
                    t,
                    &repo::TrackMeta {
                        id: id.clone(),
                        title: title.into(),
                        artists: vec![ArtistRef { id: None, name: "Probe Artist".into() }],
                        album: None,
                        track_no: None,
                        duration_ms: 200_000,
                        cover_key: None,
                    },
                )?;
                stats::record(t, &id, now_ms() - (i as i64 + 1) * 3_600_000, 180_000)?;
            }
            Ok(())
        })?;
    }
    let paired = db.read(mixtape_library::peer_store::pairing)?;
    let mut code = None;
    if paired.is_none() {
        let c = PairCode::new("Probe PC", peer::PORT, addresses, now_ms() / 1000)?;
        println!("pair uri: {}", c.to_uri());
        code = Some(c);
    } else {
        println!("already paired with {:?}", paired.map(|p| p.peer_name));
    }
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", peer::PORT)).await?;
    println!("listening on {}", listener.local_addr()?);
    loop {
        let (stream, from) = listener.accept().await?;
        let served = tokio::time::timeout(Duration::from_secs(60), peer::serve(stream, &db, code.as_ref())).await;
        match served {
            Ok(Ok(Served::Paired(p))) => {
                println!("{from}: paired with {:?}", p.peer_name);
                code = None;
            }
            Ok(Ok(other)) => println!("{from}: {other:?}"),
            Ok(Err(e)) => println!("{from}: error {e}"),
            Err(_) => println!("{from}: timed out"),
        }
        let s = db.read(|c| stats::listening(c, i64::MIN, 0))?;
        println!("  totals: {} plays, by device {:?}", s.plays, s.by_device.iter().map(|d| (&d.name, d.plays)).collect::<Vec<_>>());
    }
}
