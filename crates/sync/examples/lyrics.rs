//! Live lrclib lookup, no cache: prints what the lyrics panel would get.
//! `cargo run -p mixtape-sync --example lyrics -- <artist> <title> [album] [seconds]`

use mixtape_sync::lyrics::{Lrclib, Query, from_cache};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [artist, title, rest @ ..] = args.as_slice() else {
        eprintln!("usage: lyrics <artist> <title> [album] [seconds]");
        std::process::exit(2);
    };
    let q = Query {
        title: title.clone(),
        artist: artist.clone(),
        album: rest.first().filter(|a| !a.is_empty()).cloned(),
        duration_ms: rest.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 1000,
    };
    let found = Lrclib::new().unwrap().find(&q).await.unwrap();
    match found.and_then(|(synced, body)| from_cache(synced, Some(&body))) {
        None => println!("not found"),
        Some(l) => {
            println!(
                "synced: {}, instrumental: {}, lines: {}",
                l.synced,
                l.instrumental,
                l.lines.len()
            );
            for line in l.lines.iter().take(6) {
                println!("  {:>7} ms  {}", line.ms, line.text);
            }
            if let Some(p) = l.plain {
                println!("plain: {} chars", p.len());
            }
        }
    }
}
