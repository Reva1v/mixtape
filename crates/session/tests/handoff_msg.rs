//! The hand-off codec (contracts/handoff.md): every message round-trips, `Handoff` carries a
//! player `Snapshot` unchanged, frames are bounded lines.

use mixtape_core::TrackId;
use mixtape_library::views::ViewKind;
use mixtape_player::{Queue, QueueContext, Snapshot};
use mixtape_session::handoff::msg::hello_proto;
use mixtape_session::handoff::{Command, ErrorCode, FrameError, Handoff, MAX_FRAME, Msg, PROTO, Side, decode, encode};
use serde_json::json;

fn snapshot() -> Snapshot {
    let mut queue = Queue::new();
    queue.set((0..5).map(|i| TrackId(format!("track{i:06}"))).collect(), 2, QueueContext::None);
    Snapshot { queue, position_ms: 80_000, volume: 0.6, mix: None }
}

fn round(m: &Msg) -> Msg {
    decode(&encode(m).unwrap()).unwrap()
}

#[test]
fn handoff_carries_a_snapshot_unchanged() {
    let s = snapshot();
    let h = Handoff::from_snapshot(s.clone(), true);
    let back = match round(&Msg::Handoff { state: h }) {
        Msg::Handoff { state } => state,
        m => panic!("{m:?}"),
    };
    let (s2, playing) = back.into_snapshot();
    assert!(playing);
    assert_eq!(s2, s);
    assert_eq!(s2.queue.current().unwrap().track_id.as_str(), "track000002");
}

#[test]
fn handoff_fields_are_camel_case() {
    let line = encode(&Msg::Handoff { state: Handoff::from_snapshot(snapshot(), false) }).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(v["t"], "handoff");
    assert_eq!(v["state"]["positionMs"], 80_000);
    assert_eq!(v["state"]["playing"], false);
    assert!(v["state"]["queue"].is_object());
}

#[test]
fn every_message_round_trips() {
    let h = Handoff::from_snapshot(snapshot(), true);
    let all = [
        Msg::hello(Side::App),
        Msg::hello(Side::Island),
        Msg::Claim,
        Msg::Handoff { state: h.clone() },
        Msg::Idle,
        Msg::Ack { ok: true, reason: None },
        Msg::Ack { ok: false, reason: Some("no audio device".into()) },
        Msg::Bye { state: Some(h) },
        Msg::Bye { state: None },
        Msg::Cmd { cmd: Command::Like { id: "abcdefghijk".into() } },
        Msg::Cmd { cmd: Command::Unlike { id: "abcdefghijk".into() } },
        Msg::Cmd { cmd: Command::StartMix { seed: None } },
        Msg::Cmd { cmd: Command::StartMix { seed: Some("abcdefghijk".into()) } },
        Msg::Cmd { cmd: Command::Enqueue { ids: vec!["a".into(), "b".into()], next: true } },
        Msg::Cmd { cmd: Command::Jump { uid: 7 } },
        Msg::Cmd { cmd: Command::Remove { uid: 8 } },
        Msg::Cmd { cmd: Command::Move { uid: 9, to: 1 } },
        Msg::Cmd { cmd: Command::ShowWindow { screen: Some("login".into()) } },
        Msg::Changed { kinds: vec![ViewKind::Liked, ViewKind::Playlists] },
        Msg::Error { code: ErrorCode::Incompatible, proto: Some(PROTO) },
        Msg::Error { code: ErrorCode::Busy, proto: None },
        Msg::Error { code: ErrorCode::TooLarge, proto: None },
        Msg::Error { code: ErrorCode::BadFrame, proto: None },
    ];
    for m in all {
        let line = encode(&m).unwrap();
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(line.iter().filter(|&&b| b == b'\n').count(), 1, "one line: {m:?}");
        assert_eq!(round(&m), m);
    }
}

#[test]
fn wire_names_match_the_contract() {
    let v = |m: &Msg| serde_json::from_slice::<serde_json::Value>(&encode(m).unwrap()).unwrap();
    let hello = v(&Msg::hello(Side::App));
    assert_eq!((hello["t"].as_str(), hello["proto"].as_u64(), hello["app"].as_str()), (Some("hello"), Some(1), Some("mixtape")));
    assert_eq!(hello["pid"].as_u64(), Some(std::process::id() as u64));
    assert_eq!(v(&Msg::hello(Side::Island))["app"], "island");
    assert_eq!(v(&Msg::Claim), json!({"t": "claim"}));
    assert_eq!(v(&Msg::Error { code: ErrorCode::TooLarge, proto: None }), json!({"t": "error", "code": "too_large"}));
    assert_eq!(
        v(&Msg::Cmd { cmd: Command::StartMix { seed: None } }),
        json!({"t": "cmd", "cmd": {"op": "startMix", "seed": null}})
    );
    assert_eq!(v(&Msg::Cmd { cmd: Command::ShowWindow { screen: None } })["cmd"]["op"], "showWindow");
}

#[test]
fn proto_is_read_from_any_hello() {
    assert_eq!(PROTO, 1);
    assert_eq!(hello_proto(&encode(&Msg::hello(Side::App)).unwrap()), Some(PROTO));
    // A future peer: another proto, fields this version does not know.
    let future = br#"{"t":"hello","proto":2,"app":"mixtape","pid":1,"caps":["x"]}"#;
    assert_eq!(hello_proto(future), Some(2));
    assert_eq!(hello_proto(br#"{"t":"claim"}"#), None);
    assert_eq!(hello_proto(b"not json"), None);
}

#[test]
fn unknown_fields_are_ignored_and_new_ones_default() {
    let m = decode(b"{\"t\":\"ack\",\"ok\":true,\"extra\":{\"a\":1}}\n").unwrap();
    assert_eq!(m, Msg::Ack { ok: true, reason: None });
    // `version` is optional.
    let m = decode(b"{\"t\":\"hello\",\"proto\":1,\"app\":\"island\",\"pid\":5,\"future\":true}\n").unwrap();
    assert_eq!(m, Msg::Hello { proto: 1, app: Side::Island, pid: 5, version: String::new() });
    // CRLF ends a line too.
    assert_eq!(decode(b"{\"t\":\"idle\"}\r\n").unwrap(), Msg::Idle);
}

#[test]
fn a_frame_over_a_mebibyte_is_refused() {
    let big = Msg::Cmd { cmd: Command::Enqueue { ids: vec!["x".repeat(MAX_FRAME)], next: false } };
    assert_eq!(encode(&big), Err(FrameError::TooLarge));
    let mut line = vec![b' '; MAX_FRAME + 1];
    *line.last_mut().unwrap() = b'\n';
    assert_eq!(decode(&line), Err(FrameError::TooLarge));
}

#[test]
fn a_cut_or_broken_line_is_a_bad_frame() {
    assert!(matches!(decode(b"{\"t\":\"claim\"}"), Err(FrameError::Bad(_))), "no newline");
    assert!(matches!(decode(b"{\"t\":\"cla\n"), Err(FrameError::Bad(_))), "cut JSON");
    assert!(matches!(decode(b"{\"t\":\"dance\"}\n"), Err(FrameError::Bad(_))), "unknown message");
    assert!(matches!(decode(b"{\"t\":\"move\"}\n"), Err(FrameError::Bad(_))), "not a top-level message");
    assert_eq!(FrameError::TooLarge.code(), ErrorCode::TooLarge);
    assert_eq!(FrameError::Bad(String::new()).code(), ErrorCode::BadFrame);
}
