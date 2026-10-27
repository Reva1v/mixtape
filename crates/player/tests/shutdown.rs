//! `Player::shutdown` releases the engine and saves where playback was; a new player on the
//! same database starts from there. No audio plays and no device opens: the tracks have no
//! files, so nothing reaches the engine.

use mixtape_core::TrackId;
use mixtape_library::Db;
use mixtape_player::{Player, PlayerEvent, Queue, QueueContext, Snapshot, Status};
use std::sync::{Arc, Mutex};

fn start(db: &Db, root: &std::path::Path, events: Arc<Mutex<Vec<String>>>) -> Arc<Player> {
    let sink = Arc::new(move |ev: PlayerEvent| {
        let name = match ev {
            PlayerEvent::State(_) => "state",
            PlayerEvent::TrackChanged { .. } => "track",
            PlayerEvent::QueueChanged => "queue",
            PlayerEvent::Played { .. } => "played",
            PlayerEvent::Listened(_) => "listened",
            PlayerEvent::Error(_) => "error",
        };
        events.lock().unwrap().push(name.into());
    });
    Player::start(db.clone(), root.to_owned(), sink, None).expect("player starts")
}

fn snapshot() -> Snapshot {
    let mut queue = Queue::new();
    queue.set((0..3).map(|i| TrackId(format!("local{i:011}"))).collect(), 1, QueueContext::None);
    Snapshot { queue, position_ms: 42_000, volume: 0.4, mix: None }
}

#[test]
fn shutdown_stops_the_engine_and_a_new_start_resumes_the_position() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("mixtape.db")).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));

    let player = start(&db, dir.path(), events.clone());
    assert!(!player.is_shut_down());
    player.restore_snapshot(snapshot(), false);
    let s = player.state();
    assert_eq!(s.track_id.as_ref().map(|t| t.as_str()), Some("local00000000001"));
    assert_eq!((s.status, s.position_ms), (Status::Paused, 42_000));
    assert!(events.lock().unwrap().iter().any(|e| e == "queue"));
    let (snap, playing) = player.snapshot();
    assert!(!playing);
    assert_eq!((snap.position_ms, snap.volume), (42_000, 0.4));

    player.shutdown();
    assert!(player.is_shut_down());
    // Commands after the release are ignored, not a panic.
    player.play();
    player.seek(1_000);
    player.shutdown();
    drop(player);

    let again = start(&db, dir.path(), Arc::new(Mutex::new(Vec::new())));
    let s = again.state();
    assert_eq!(s.track_id.as_ref().map(|t| t.as_str()), Some("local00000000001"));
    assert_eq!(s.position_ms, 42_000);
    assert!((s.volume - 0.4).abs() < 1e-6);
    let (snap, _) = again.snapshot();
    assert_eq!(snap.queue, snapshot().queue);
    again.shutdown();
}

#[test]
fn restoring_an_empty_snapshot_goes_idle() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("mixtape.db")).unwrap();
    let player = start(&db, dir.path(), Arc::new(Mutex::new(Vec::new())));
    player.restore_snapshot(Snapshot { queue: Queue::new(), position_ms: 0, volume: 0.8, mix: None }, true);
    assert_eq!(player.state().status, Status::Idle);
    assert_eq!(player.state().track_id, None);
    player.shutdown();
}
