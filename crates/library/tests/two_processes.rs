//! mixtape and the island's music module each open `mixtape.db` with their own `Db`
//! (one writer and its readers per process). WAL and `busy_timeout` must let both write and
//! see each other's writes without a restart.

use mixtape_core::{ArtistId, ArtistRef, TrackId};
use mixtape_library::repo::{self, TrackMeta};
use mixtape_library::{Db, likes};

fn meta(id: &str) -> TrackMeta {
    TrackMeta {
        id: TrackId::from(id),
        title: format!("Track {id}"),
        artists: vec![ArtistRef { id: Some(ArtistId::from("UCartist")), name: "Artist".into() }],
        album: None,
        track_no: None,
        duration_ms: 180_000,
        cover_key: None,
    }
}

fn liked(db: &Db, id: &str) -> bool {
    db.read(|c| repo::track(c, &TrackId::from(id))).unwrap().expect("track exists").liked
}

#[test]
fn a_like_written_by_one_is_read_by_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixtape.db");
    let app = Db::open(&path).unwrap();
    let island = Db::open(&path).unwrap();
    // A local id: the like is stored directly, not queued for YouTube Music.
    app.write(|t| repo::upsert_track(t, &meta("local0000000001"))).unwrap();
    assert!(!liked(&island, "local0000000001"));

    island.write(|t| likes::set_like(t, &TrackId::from("local0000000001"), true)).unwrap();
    assert!(liked(&app, "local0000000001"));

    app.write(|t| likes::set_like(t, &TrackId::from("local0000000001"), false)).unwrap();
    assert!(!liked(&island, "local0000000001"));
}

#[test]
fn both_write_at_once_without_database_is_locked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixtape.db");
    let a = Db::open(&path).unwrap();
    let b = Db::open(&path).unwrap();
    let ids: Vec<String> = (0..40).map(|i| format!("local{i:011}")).collect();
    a.write({
        let ids = ids.clone();
        move |t| {
            for id in &ids {
                repo::upsert_track(t, &meta(id))?;
            }
            Ok(())
        }
    })
    .unwrap();

    let run = |db: Db, ids: Vec<String>, on: bool| {
        std::thread::spawn(move || {
            for id in ids {
                db.write(move |t| likes::set_like(t, &TrackId::from(id.as_str()), on)).expect("write");
            }
        })
    };
    let (even, odd): (Vec<_>, Vec<_>) = ids.iter().cloned().enumerate().partition(|(i, _)| i % 2 == 0);
    let even: Vec<String> = even.into_iter().map(|(_, id)| id).collect();
    let odd: Vec<String> = odd.into_iter().map(|(_, id)| id).collect();
    let ta = run(a.clone(), even.clone(), true);
    let tb = run(b.clone(), odd.clone(), true);
    ta.join().unwrap();
    tb.join().unwrap();

    for id in even.iter().chain(&odd) {
        assert!(liked(&a, id), "{id} liked, seen by a");
        assert!(liked(&b, id), "{id} liked, seen by b");
    }
}
