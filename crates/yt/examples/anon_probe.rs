//! Live check without an account: `cargo run -p mixtape-yt --example anon_probe`.
//! Prints chip titles, shelf titles with item counts and a radio's size. Read-only.

use mixtape_core::TrackId;
use mixtape_yt::innertube::InnerTube;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let yt = InnerTube::new("ru", "RU")?;
    let feed = yt.home_feed(None, 12).await?;
    println!(
        "chips: {:?}",
        feed.chips.iter().map(|c| &c.title).collect::<Vec<_>>()
    );
    for s in &feed.shelves {
        println!("shelf {:?}: {}", s.title, s.items.len());
    }
    if let Some(chip) = feed.chips.first() {
        let mood = yt.home_feed(Some(&chip.params), 6).await?;
        println!(
            "chip {:?}: {} shelves, first {:?}",
            chip.title,
            mood.shelves.len(),
            mood.shelves.first().map(|s| &s.title)
        );
    }
    let radio = yt.radio(&TrackId::new("4NRXx6U8ABQ")).await?;
    println!(
        "radio: {} tracks, {} chips, continuation {}",
        radio.tracks.len(),
        radio.chips.len(),
        radio.continuation.is_some()
    );
    Ok(())
}
