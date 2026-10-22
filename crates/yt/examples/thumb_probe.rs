//! Prints card thumbnail URLs of the anonymous home feed and mood feeds that the small-cover
//! downloader would reject: `cargo run -p mixtape-yt --example thumb_probe`.
use mixtape_yt::home::HomeItem;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::thumbs::small_candidates;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let yt = InnerTube::new("ru", "RU")?;
    let mut feeds = vec![yt.home_feed(None, 12).await?];
    for c in feeds[0].chips.clone().iter().take(4) {
        feeds.push(yt.home_feed(Some(&c.params), 6).await?);
    }
    let mut bad = 0;
    let mut total = 0;
    for f in &feeds {
        for s in &f.shelves {
            for i in &s.items {
                let (kind, url) = match i {
                    HomeItem::Mix { thumbnail, .. } => ("mix", thumbnail.clone()),
                    HomeItem::Album(a) => ("album", a.thumbnail.clone()),
                    HomeItem::Playlist(p) => ("playlist", p.thumbnail.clone()),
                    HomeItem::Artist(a) => ("artist", a.thumbnail.clone()),
                    HomeItem::Track(t) => ("track", t.thumbnail.clone()),
                };
                total += 1;
                match url {
                    None => { bad += 1; println!("{kind}: no thumbnail"); }
                    Some(u) if small_candidates(&u).is_empty() => { bad += 1; println!("{kind}: rejected {u}"); }
                    _ => {}
                }
            }
        }
    }
    println!("{bad} of {total} cards without a usable thumbnail");
    Ok(())
}
