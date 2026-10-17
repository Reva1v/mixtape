//! Live check of the Spotify import without the app or a database: reads a public playlist
//! or album (or a text file of «Artist — Title» lines) and matches it on YouTube Music.
//! Prints each match with its score and the tracks not found; never prints cookie values.
//! `cargo run -p mixtape-sync --example spotify_match -- <cookies.txt> <spotify link | lines.txt> [limit]`

use mixtape_sync::spotify::{self, import, matching, page};
use mixtape_yt::auth::Cookies;
use mixtape_yt::innertube::InnerTube;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: spotify_match <cookies.txt> <spotify link | lines.txt> [limit]".into());
    }
    let limit: usize = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let yt = Arc::new(InnerTube::new("en", "US")?);
    yt.set_cookies(Some(Cookies::parse(&std::fs::read_to_string(&args[1])?)?));

    let (name, mut tracks) = match spotify::parse_link(&args[2]) {
        Some(link) => {
            let list = page::fetch(yt.http(), &link).await?;
            println!(
                "{:?} «{}»: read {}, total {:?}, cut {}",
                list.kind,
                list.name,
                list.tracks.len(),
                list.total,
                list.is_cut()
            );
            (list.name, list.tracks)
        }
        None => (
            "lines".to_owned(),
            spotify::parse_lines(&std::fs::read_to_string(&args[2])?),
        ),
    };
    tracks.truncate(limit);
    let started = std::time::Instant::now();
    let found = import::match_all(
        yt,
        &tracks,
        Arc::new(import::pacer()),
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    )
    .await?;
    for (t, f) in tracks.iter().zip(&found) {
        match f {
            Some(y) => {
                let score = matching::score(t, y).unwrap_or_default();
                let artists: Vec<&str> = y.artists.iter().map(|a| a.name.as_str()).collect();
                let dt =
                    (y.duration_ms as i64 - t.duration_ms.unwrap_or(y.duration_ms) as i64) / 1000;
                println!(
                    "  {score:.2}  {}  =>  {} — {} [{:?} {dt:+}s] {}",
                    t.line(),
                    artists.join(", "),
                    y.title,
                    y.kind,
                    y.id
                );
            }
            None => println!("  ----  {}  =>  NOT FOUND", t.line()),
        }
    }
    let s = import::summary(&name, &tracks, &found, None, None);
    println!(
        "matched {} of {} in {:.1} s",
        s.matched,
        s.total,
        started.elapsed().as_secs_f32()
    );
    Ok(())
}
