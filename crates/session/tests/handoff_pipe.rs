//! The hand-off transport in one process: real pipes under test names, plus in-memory
//! streams for peers that misbehave.

#![cfg(windows)]

use mixtape_player::{Queue, Snapshot};
use mixtape_session::handoff::{
    ErrorCode, FrameError, Handoff, Link, LinkError, MAX_FRAME, Msg, PROTO, Server, Side, TIMEOUT, keep_new,
    pipe_name,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

fn test_pipe() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    format!(r"\\.\pipe\mixtape-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

/// Serves `name` as `me`; every accepted link arrives on the channel.
fn serve(name: &str, me: Side) -> (Server, mpsc::UnboundedReceiver<Link>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let server = Server::serve(&tokio::runtime::Handle::current(), name, me, move |link| {
        let _ = tx.send(link);
    })
    .expect("serve");
    (server, rx)
}

async fn accepted(rx: &mut mpsc::UnboundedReceiver<Link>) -> Link {
    tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("accepted in time").expect("server alive")
}

fn handoff() -> Handoff {
    Handoff::from_snapshot(Snapshot { queue: Queue::new(), position_ms: 80_000, volume: 0.7, mix: None }, true)
}

#[test]
fn pipe_names_carry_the_user_sid() {
    let app = pipe_name(Side::App).unwrap();
    let island = pipe_name(Side::Island).unwrap();
    assert!(app.starts_with(r"\\.\pipe\mixtape-S-1-"), "{app}");
    assert!(app.ends_with("-app"), "{app}");
    assert!(island.ends_with("-island"), "{island}");
    assert_eq!(app.trim_end_matches("-app"), island.trim_end_matches("-island"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_tells_each_side_who_the_other_is() {
    let name = test_pipe();
    let (_server, mut links) = serve(&name, Side::Island);
    let client = Link::connect(&name, Side::App).await.unwrap();
    let server_side = accepted(&mut links).await;
    assert_eq!(client.peer().side, Side::Island);
    assert_eq!(client.peer().pid, std::process::id());
    assert_eq!(server_side.peer().side, Side::App);
    // The app connected, on both ends.
    assert_eq!(client.opened_by(), Side::App);
    assert_eq!(server_side.opened_by(), Side::App);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_server_is_no_peer() {
    assert_eq!(Link::connect(&test_pipe(), Side::App).await.err(), Some(LinkError::NoPeer));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_server_of_one_name_fails() {
    let name = test_pipe();
    let (_server, _links) = serve(&name, Side::App);
    assert!(Server::serve(&tokio::runtime::Handle::current(), &name, Side::App, |_| {}).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_handoff_ack() {
    let name = test_pipe();
    let (_server, mut links) = serve(&name, Side::Island);
    let mut app = Link::connect(&name, Side::App).await.unwrap();
    let mut island = accepted(&mut links).await;

    let island_task = tokio::spawn(async move {
        assert_eq!(island.recv().await.unwrap(), Msg::Claim);
        let answer = island.request(&Msg::Handoff { state: handoff() }).await.unwrap();
        assert_eq!(answer, Msg::Ack { ok: true, reason: None });
    });
    let got = app.request(&Msg::Claim).await.unwrap();
    assert_eq!(got, Msg::Handoff { state: handoff() });
    app.send(&Msg::Ack { ok: true, reason: None }).await.unwrap();
    island_task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_idle() {
    let name = test_pipe();
    let (_server, mut links) = serve(&name, Side::Island);
    let mut app = Link::connect(&name, Side::App).await.unwrap();
    let mut island = accepted(&mut links).await;
    let t = tokio::spawn(async move {
        assert_eq!(island.recv().await.unwrap(), Msg::Claim);
        island.send(&Msg::Idle).await.unwrap();
    });
    assert_eq!(app.request(&Msg::Claim).await.unwrap(), Msg::Idle);
    t.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bye_then_the_end() {
    let name = test_pipe();
    let (_server, mut links) = serve(&name, Side::Island);
    let app = Link::connect(&name, Side::App).await.unwrap();
    let mut island = accepted(&mut links).await;
    app.send(&Msg::Bye { state: Some(handoff()) }).await.unwrap();
    drop(app);
    assert_eq!(island.recv().await.unwrap(), Msg::Bye { state: Some(handoff()) });
    assert_eq!(island.recv().await, Err(LinkError::Closed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_drop_without_bye_reads_as_closed() {
    let name = test_pipe();
    let (_server, mut links) = serve(&name, Side::Island);
    let app = Link::connect(&name, Side::App).await.unwrap();
    let mut island = accepted(&mut links).await;
    drop(app);
    assert_eq!(island.recv_timeout(Duration::from_secs(2)).await, Err(LinkError::Closed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_client_is_busy_until_the_first_leaves() {
    let name = test_pipe();
    let (_server, mut links) = serve(&name, Side::Island);
    let first = Link::connect(&name, Side::App).await.unwrap();
    let held = accepted(&mut links).await;
    assert_eq!(Link::connect(&name, Side::App).await.err(), Some(LinkError::Busy));
    drop(first);
    drop(held);
    let again = Link::connect(&name, Side::App).await;
    assert!(again.is_ok(), "{:?}", again.err());
    accepted(&mut links).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silence_times_out_in_500_ms() {
    let (ours, _theirs) = tokio::io::duplex(4096);
    let t0 = Instant::now();
    assert_eq!(Link::over(ours, Side::App, Side::App).await.err(), Some(LinkError::Timeout));
    let took = t0.elapsed();
    assert!(took >= TIMEOUT && took < TIMEOUT * 3, "{took:?}");
    assert_eq!(TIMEOUT, Duration::from_millis(500));

    // An answer that never comes.
    let name = test_pipe();
    let (_server, mut links) = serve(&name, Side::Island);
    let mut app = Link::connect(&name, Side::App).await.unwrap();
    let _island = accepted(&mut links).await;
    assert_eq!(app.request(&Msg::Claim).await, Err(LinkError::Timeout));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_proto_is_incompatible_and_told_so() {
    let (ours, theirs) = tokio::io::duplex(4096);
    let peer = tokio::spawn(async move {
        let (r, mut w) = tokio::io::split(theirs);
        w.write_all(b"{\"t\":\"hello\",\"proto\":2,\"app\":\"island\",\"pid\":1,\"version\":\"9\"}\n").await.unwrap();
        let mut lines = BufReader::new(r).lines();
        let hello = lines.next_line().await.unwrap().unwrap();
        let error = lines.next_line().await.unwrap().unwrap();
        (hello, error)
    });
    assert_eq!(Link::over(ours, Side::App, Side::App).await.err(), Some(LinkError::Incompatible(2)));
    let (hello, error) = peer.await.unwrap();
    assert!(hello.contains("\"t\":\"hello\""));
    let e: Msg = serde_json::from_str(&error).unwrap();
    assert_eq!(e, Msg::Error { code: ErrorCode::Incompatible, proto: Some(PROTO) });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_frame_ends_the_link() {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let peer = tokio::spawn(async move {
        let (r, mut w) = tokio::io::split(theirs);
        w.write_all(b"{\"t\":\"hello\",\"proto\":1,\"app\":\"island\",\"pid\":1}\n").await.unwrap();
        let mut lines = BufReader::new(r).lines();
        let _hello = lines.next_line().await.unwrap();
        w.write_all(&vec![b'x'; MAX_FRAME + 10]).await.unwrap();
        lines.next_line().await.unwrap().unwrap()
    });
    let mut link = Link::over(ours, Side::App, Side::App).await.unwrap();
    assert_eq!(link.recv().await, Err(LinkError::Frame(FrameError::TooLarge)));
    let e: Msg = serde_json::from_str(&peer.await.unwrap()).unwrap();
    assert_eq!(e, Msg::Error { code: ErrorCode::TooLarge, proto: None });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_error_ends_the_link() {
    let (ours, theirs) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let (_r, mut w) = tokio::io::split(theirs);
        w.write_all(b"{\"t\":\"hello\",\"proto\":1,\"app\":\"island\",\"pid\":1}\n{\"t\":\"error\",\"code\":\"bad_frame\"}\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let mut link = Link::over(ours, Side::App, Side::App).await.unwrap();
    assert_eq!(link.recv().await, Err(LinkError::Peer(ErrorCode::BadFrame)));
}

#[test]
fn of_two_links_the_one_the_app_opened_stays() {
    // The island reached the app's server first, then the app reached the island's.
    assert!(keep_new(Side::Island, Side::App));
    // The other order: the app's link is already there.
    assert!(!keep_new(Side::App, Side::Island));
    assert!(!keep_new(Side::App, Side::App));
    assert!(!keep_new(Side::Island, Side::Island));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_directions_at_once_keep_one_link() {
    let (app_pipe, island_pipe) = (test_pipe(), test_pipe());
    let (_app_server, mut app_links) = serve(&app_pipe, Side::App);
    let (_island_server, mut island_links) = serve(&island_pipe, Side::Island);
    // Both start together: each connects to the other's server.
    let (from_island, from_app) =
        tokio::join!(Link::connect(&app_pipe, Side::Island), Link::connect(&island_pipe, Side::App));
    let (from_island, from_app) = (from_island.unwrap(), from_app.unwrap());
    let at_app = accepted(&mut app_links).await;
    let at_island = accepted(&mut island_links).await;

    // The island holds `from_island` (opened by itself) and receives `at_island` (opened by
    // the app): it keeps the latter. The app does the same the other way round.
    assert!(keep_new(from_island.opened_by(), at_island.opened_by()));
    assert!(!keep_new(from_app.opened_by(), at_app.opened_by()));
    drop(from_island);
    let mut at_app = at_app;
    assert_eq!(at_app.recv_timeout(Duration::from_secs(2)).await, Err(LinkError::Closed));
    // The kept link works.
    let mut at_island = at_island;
    from_app.send(&Msg::Claim).await.unwrap();
    assert_eq!(at_island.recv().await.unwrap(), Msg::Claim);
}
