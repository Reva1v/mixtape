//! Live check against a real account: `cargo run -p mixtape-yt --example probe -- <cookies.txt>`.
//! Prints counts and a few titles; never prints cookie values.

use mixtape_yt::api::YtMusic;
use mixtape_yt::auth::Cookies;
use mixtape_yt::innertube::InnerTube;
use std::time::Instant;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: probe <cookies.txt>")?;
    let yt = InnerTube::new("ru", "RU")?;
    yt.set_cookies(Some(Cookies::parse(&std::fs::read_to_string(path)?)?));

    let t = Instant::now();
    let acc = yt.account().await?;
    println!(
        "account: {} {:?} avatar={} ({:?})",
        acc.name,
        acc.handle,
        acc.avatar.is_some(),
        t.elapsed()
    );

    let t = Instant::now();
    let liked = yt.liked_tracks().await?;
    println!("liked: {} ({:?})", liked.len(), t.elapsed());
    for x in liked.iter().take(3) {
        println!(
            "  {} — {:?} | {:?} | {} ms | {:?} | thumb={}",
            x.title,
            x.artists.iter().map(|a| &a.name).collect::<Vec<_>>(),
            x.album.as_ref().map(|a| &a.title),
            x.duration_ms,
            x.kind,
            x.thumbnail.is_some()
        );
    }
    let no_album = liked.iter().filter(|x| x.album.is_none()).count();
    let videos = liked
        .iter()
        .filter(|x| x.kind != mixtape_yt::api::VideoKind::Audio)
        .count();
    println!("  without album: {no_album}, not audio: {videos}");

    let pls = yt.library_playlists().await?;
    println!("playlists: {}", pls.len());
    for p in &pls {
        println!(
            "  {} | {} | {:?} | {:?}",
            p.id, p.title, p.author, p.track_count
        );
    }
    let albums = yt.library_albums().await?;
    println!("albums: {}", albums.len());
    for a in albums.iter().take(5) {
        println!(
            "  {} | {} | {:?} | {:?} | {:?}",
            a.id,
            a.title,
            a.artists.iter().map(|a| &a.name).collect::<Vec<_>>(),
            a.year,
            a.album_type
        );
    }
    let artists = yt.library_artists().await?;
    println!("artists: {}", artists.len());
    for a in artists.iter().take(5) {
        println!("  {} | {}", a.id, a.name);
    }
    if let Some(p) = pls.first() {
        let t = Instant::now();
        let full = yt.playlist(&p.id).await?;
        println!(
            "playlist {}: {} tracks (header says {:?}) ({:?})",
            full.card.title,
            full.tracks.len(),
            full.card.track_count,
            t.elapsed()
        );
    }
    if let Some(a) = albums.first() {
        let full = yt.album(&a.id).await?;
        println!(
            "album {}: {} tracks, olak={}, type {:?}",
            full.card.title,
            full.tracks.len(),
            full.playlist_id.is_some(),
            full.card.album_type
        );
        for x in full.tracks.iter().take(2) {
            println!(
                "  #{:?} {} — {:?}",
                x.track_no,
                x.title,
                x.artists.iter().map(|a| &a.name).collect::<Vec<_>>()
            );
        }
        if let Some(ar) = full.card.artists.iter().find_map(|a| a.id.clone()) {
            let artist = yt.artist(&ar).await?;
            println!(
                "artist {}: top {} discography {}",
                artist.card.name,
                artist.top_tracks.len(),
                artist.discography.len()
            );
        }
    }
    let big = mixtape_core::ArtistId::new("UCqMDNf3Pn5L7pcNkuSEeO3w"); // a large public discography
    let artist = yt.artist(&big).await?;
    println!(
        "artist {}: top {} discography {}",
        artist.card.name,
        artist.top_tracks.len(),
        artist.discography.len()
    );
    for a in artist.discography.iter().take(4) {
        println!("  {} {:?} {:?}", a.title, a.year, a.album_type);
    }
    let one = yt.track(&liked[0].id).await?;
    println!(
        "track: {} — {:?} | {:?}",
        one.title,
        one.artists.iter().map(|a| &a.name).collect::<Vec<_>>(),
        one.album.as_ref().map(|a| &a.title)
    );
    Ok(())
}
