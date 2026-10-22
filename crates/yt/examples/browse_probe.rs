//! Live check of browse pages without an account:
//! `cargo run -p mixtape-yt --example browse_probe -- FEmusic_charts [params]`.
//! Prints shelf titles and item kinds parsed with the home parser, and the grid items count.

use mixtape_yt::home::{self, HomeItem};
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::parse::find_all;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let yt = InnerTube::new("ru", "RU")?;
    let resp = match args.get(1) {
        Some(p) => yt.browse_params(&args[0], p).await?,
        None => yt.browse(&args[0]).await?,
    };
    let page = home::parse_page(&resp);
    for s in &page.shelves {
        let kinds: Vec<&str> = s.items.iter().map(|i| match i {
            HomeItem::Track(_) => "track",
            HomeItem::Album(_) => "album",
            HomeItem::Playlist(_) => "playlist",
            HomeItem::Artist(_) => "artist",
            HomeItem::Mix { .. } => "mix",
        }).collect();
        println!("shelf {:?}: {:?}", s.title, kinds);
    }
    println!("continuation: {}", page.continuation.is_some());
    if let Some(sections) = mixtape_yt::parse::find_first(&resp, "sectionListRenderer").and_then(|s| s.get("contents")).and_then(|c| c.as_array()) {
        for s in sections {
            let key = s.as_object().and_then(|o| o.keys().next().cloned()).unwrap_or_default();
            let title = mixtape_yt::parse::find_first(s, "title").map(|t| mixtape_yt::parse::text(Some(t))).unwrap_or_default();
            println!("section {key} {title:?}");
        }
    }
    for key in ["gridRenderer", "musicNavigationButtonRenderer", "musicResponsiveListItemRenderer", "musicTwoRowItemRenderer"] {
        println!("{key}: {}", find_all(&resp, key).len());
    }
    for b in find_all(&resp, "musicNavigationButtonRenderer").iter().take(6) {
        println!("  button {:?} -> {}", mixtape_yt::parse::text(b.get("buttonText")), b.pointer("/clickCommand/browseEndpoint").map(|v| v.to_string()).unwrap_or_default());
    }
    Ok(())
}
