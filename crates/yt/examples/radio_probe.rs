//! Live check of the radio API on a public seed:
//! `cargo run -p mixtape-yt --example radio_probe -- <cookies.txt> [fixtures-dir]`.
//! Prints counts and chip titles; never prints cookie values. With `fixtures-dir`, writes the
//! `next` responses there (minified), scrubbed of account data and trimmed to what the parser reads.
//! Never calls `set_like`: it would change the account's real likes.

use mixtape_core::TrackId;
use mixtape_yt::api::VideoKind;
use mixtape_yt::auth::Cookies;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::radio::{self, RadioPage};
use serde_json::Value;
use std::collections::HashSet;

/// Keys that are noise for the parser or tied to the session/account.
const DROP: &[&str] = &[
    "menu",
    "trackingParams",
    "clickTrackingParams",
    "accessibility",
    "accessibilityData",
    "loggingContext",
    "queueNavigationEndpoint",
    "playlistEditParams",
    "playlistSetVideoId",
    "shortBylineText",
    "canReorder",
    "segmentMap",
    "shuffleToggleButton",
    "playerParams",
    "hack",
    "musicQueueConfig",
];

fn scrub(v: &mut Value) {
    match v {
        Value::Object(o) => {
            o.retain(|k, _| !DROP.contains(&k.as_str()));
            o.values_mut().for_each(scrub);
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        _ => {}
    }
}

/// Only the queue survives: no responseContext (visitorData, logged-in flags), overlays or
/// like buttons.
fn fixture(mut resp: Value) -> Value {
    if let Value::Object(o) = &mut resp {
        o.retain(|k, _| k == "contents" || k == "continuationContents");
    }
    scrub(&mut resp);
    resp
}

fn summary(label: &str, page: &RadioPage) {
    let audio = page
        .tracks
        .iter()
        .filter(|t| t.kind == VideoKind::Audio)
        .count();
    println!(
        "{label}: {} tracks ({audio} audio), playlist {:?}, continuation={}, chips={}",
        page.tracks.len(),
        page.playlist_id,
        page.continuation.is_some(),
        page.chips.len()
    );
    for t in page.tracks.iter().take(3) {
        let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
        println!(
            "  {} {} — {artists:?} | {:?}",
            t.id,
            t.title,
            t.album.as_ref().map(|a| &a.title)
        );
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: radio_probe <cookies.txt> [fixtures-dir]")?;
    let out = std::env::args().nth(2);
    let yt = InnerTube::new("ru", "RU")?;
    yt.set_cookies(Some(Cookies::parse(&std::fs::read_to_string(path)?)?));
    let save = |name: &str, resp: Value| -> Result<(), Box<dyn std::error::Error>> {
        if let Some(dir) = &out {
            let text = serde_json::to_string(&fixture(resp))?;
            println!("  -> {name}: {} KB", text.len() / 1024);
            std::fs::write(format!("{dir}/{name}"), text)?;
        }
        Ok(())
    };
    let seed = TrackId::new("lYBUbBu4W08"); // Rick Astley, the album version

    let raw = yt.call("next", radio::radio_body(&seed)).await?;
    let first = radio::parse_page(&raw).ok_or("no queue in the first page")?;
    summary("radio", &first);
    for c in &first.chips {
        println!(
            "  chip {}{} id={}",
            c.title,
            if c.selected { " [selected]" } else { "" },
            c.id
        );
    }
    save("next_radio.json", raw)?;

    let pl = first.playlist_id.clone().unwrap_or_default();
    let token = first.continuation.clone().ok_or("no continuation")?;
    let raw = yt.call("next", radio::more_body(&pl, &token)).await?;
    let more = radio::parse_page(&raw).ok_or("no queue in the continuation")?;
    summary("more", &more);
    let seen: HashSet<&str> = first.tracks.iter().map(|t| t.id.as_str()).collect();
    println!(
        "  repeats from page 1: {}",
        more.tracks
            .iter()
            .filter(|t| seen.contains(t.id.as_str()))
            .count()
    );
    save("next_radio_more.json", raw)?;

    let chip = first
        .chips
        .iter()
        .find(|c| c.title == "Популярное")
        .or(first.chips.get(2))
        .ok_or("no chips")?;
    let raw = yt.call("next", radio::chip_body(&seed, chip)).await?;
    let by_chip = radio::parse_page(&raw).ok_or("no queue for the chip")?;
    summary(&format!("chip {}", chip.title), &by_chip);
    save("next_radio_chip.json", raw)?;

    // The public methods end to end.
    let page = yt.radio(&seed).await?;
    summary("InnerTube::radio", &page);
    let next = yt
        .radio_more(
            page.playlist_id.as_deref().unwrap_or(&pl),
            page.continuation.as_deref().unwrap_or(""),
        )
        .await?;
    summary("InnerTube::radio_more", &next);
    let chip_page = yt.radio_by_chip(&seed, chip).await?;
    summary("InnerTube::radio_by_chip", &chip_page);
    if let (Some(p), Some(c)) = (&chip_page.playlist_id, &chip_page.continuation) {
        summary("radio_more on the chip", &yt.radio_more(p, c).await?);
    }
    Ok(())
}
