//! Live check of the yt-dlp-free resolver on public videos:
//! `cargo run -p mixtape-yt --example direct_probe -- <videoId>…`.
//! For each id prints the track, then what every built-in client answers (itag, size, URL
//! lifetime) and which one the resolver settles on. Never downloads the audio.

use std::time::{SystemTime, UNIX_EPOCH};

use mixtape_core::TrackId;
use mixtape_yt::direct::{DEFAULT_CLIENTS, DirectResolver, parse_clients};
use mixtape_yt::innertube::InnerTube;

#[tokio::main]
async fn main() {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    if ids.is_empty() {
        eprintln!("usage: direct_probe <videoId>…");
        std::process::exit(2);
    }
    let it = InnerTube::new("en", "UA").expect("http client");
    let clients = parse_clients(DEFAULT_CLIENTS).expect("clients.json");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for raw in ids {
        let id = TrackId::new(raw.as_str());
        println!("== {id}");
        let mut visitor = None;
        match it.track_info(&id).await {
            Ok(info) => {
                let t = &info.track;
                let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
                println!(
                    "   {} — {} [{}] {} ms, cover: {}, visitorData: {}",
                    t.title,
                    artists.join(", "),
                    t.album.as_ref().map_or("-", |a| a.title.as_str()),
                    t.duration_ms,
                    t.thumbnail.is_some(),
                    info.visitor_data.is_some()
                );
                visitor = info.visitor_data;
            }
            Err(e) => println!("   next: {e}"),
        }
        for c in &clients {
            let one = DirectResolver::new(it.http().clone(), vec![c.clone()], "en", "UA");
            one.set_visitor_data(visitor.clone());
            match one.resolve_with_client(&id).await {
                Ok((s, _)) => println!(
                    "   {:<16} itag {} {} {:?} bytes, lives {} min",
                    c.name,
                    s.itag,
                    s.container.as_str(),
                    s.size,
                    s.expires_at.saturating_sub(now) / 60
                ),
                Err(e) => println!("   {:<16} {e}", c.name),
            }
        }
        let all = DirectResolver::new(it.http().clone(), clients.clone(), "en", "UA");
        all.set_visitor_data(visitor);
        match all.resolve_with_client(&id).await {
            Ok((_, client)) => println!("   -> {client}"),
            Err(e) => println!("   -> {e}"),
        }
    }
}
