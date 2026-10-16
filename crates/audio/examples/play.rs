//! Plays files through the real engine and prints its events:
//! `cargo run -p mixtape-audio --example play -- <file> [<file2> ...]`.
//!
//! Later files are queued as gapless successors when the engine asks for them. Keys: Enter
//! toggles pause, `s<sec>` seeks, `q` quits. Exits by itself when playback ends.

use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Instant;

use mixtape_audio::{Engine, EngineEvent, Source, gain_for, measure_loudness};

fn source(path: &str) -> Source {
    let gain = match measure_loudness(Path::new(path)) {
        Ok(l) => {
            let gain = gain_for(Some(l.lufs), Some(l.true_peak));
            println!(
                "{path}: {:.1} LUFS, {:.1} dBTP → gain {gain:+.1} dB",
                l.lufs, l.true_peak
            );
            gain
        }
        Err(e) => {
            println!("{path}: loudness unavailable ({e})");
            0.0
        }
    };
    Source::file(path, path, gain)
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = files.first() else {
        eprintln!("usage: play <file> [<file2> ...]");
        std::process::exit(2);
    };
    let (engine, events) = Engine::start().expect("audio engine");
    let first = source(first);
    let t0 = Instant::now();
    engine.load(first, 0, true);
    let mut queue = files[1..].iter();

    let (keys_tx, mut keys) = crossbeam_channel::unbounded::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if keys_tx.send(line).is_err() {
                break;
            }
        }
    });

    let (mut started_any, mut just_ended) = (false, false);
    loop {
        crossbeam_channel::select! {
            recv(events) -> ev => {
                let Ok(ev) = ev else { break };
                match &ev {
                    EngineEvent::Position { id, ms } => {
                        print!("\r{id} {:>3}:{:02}.{:03}   ", ms / 60_000, ms / 1000 % 60, ms % 1000);
                        let _ = std::io::stdout().flush();
                    }
                    other => println!("\n[{:>5} ms] {other:?}", t0.elapsed().as_millis()),
                }
                match ev {
                    EngineEvent::NeedNext { .. } => {
                        if let Some(f) = queue.next() {
                            engine.set_next(Some(source(f)));
                        }
                    }
                    EngineEvent::Started { .. } => (started_any, just_ended) = (true, false),
                    EngineEvent::Ended { completed: true, .. } => just_ended = true,
                    EngineEvent::StateChanged { playing: false } if just_ended => break,
                    EngineEvent::Error { .. } if !started_any => break,
                    _ => {}
                }
            }
            recv(keys) -> key => match key.as_deref().map(str::trim) {
                // No console (stdin closed): just play to the end.
                Err(_) => keys = crossbeam_channel::never(),
                Ok("q") => break,
                Ok(s) if s.starts_with('s') => {
                    if let Ok(sec) = s[1..].trim().parse::<f64>() {
                        engine.seek((sec * 1000.0) as u64);
                    }
                }
                Ok(_) => engine.toggle(),
            },
        }
    }
    engine.stop();
    println!();
}
