//! Live check of catalog search on neutral queries:
//! `cargo run -p mixtape-yt --example search_probe -- <cookies.txt> [fixtures-dir]`.
//! Prints counts and a few rows; never prints cookie values. With `fixtures-dir`, writes the
//! responses there (minified), scrubbed of account data and trimmed to what the parser reads.
//! Calls only read endpoints.

use mixtape_yt::auth::Cookies;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::search::{self, SearchItem, SearchKind, SearchPage};
use serde_json::{Value, json};

/// Keys that are noise for the parser or tied to the session/account.
const DROP: &[&str] = &[
    "trackingParams",
    "clickTrackingParams",
    "accessibility",
    "accessibilityData",
    "loggingContext",
    "overlay",
    "buttons",
    "chipCloudRenderer",
    "serviceEndpoint",
    "feedbackToken",
    "playerParams",
    "thumbnailOverlay",
    "endIcon",
    "shelfDivider",
    "displayPriority",
];

/// The "…" menu keeps only its browse links (artist, album), which the parser reads.
fn menu(v: &Value) -> Value {
    let items: Vec<Value> = v
        .pointer("/menuRenderer/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let b = i.pointer("/menuNavigationItemRenderer/navigationEndpoint/browseEndpoint")?;
            let mut b = b.clone();
            scrub(&mut b);
            Some(json!({"menuNavigationItemRenderer": {"navigationEndpoint": {"browseEndpoint": b}}}))
        })
        .collect();
    json!({"menuRenderer": {"items": items}})
}

fn scrub(v: &mut Value) {
    match v {
        Value::Object(o) => {
            o.retain(|k, _| !DROP.contains(&k.as_str()));
            if let Some(m) = o.get("menu").map(menu) {
                o.insert("menu".into(), m);
            }
            o.iter_mut()
                .filter(|(k, _)| k.as_str() != "menu")
                .for_each(|(_, c)| scrub(c));
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        _ => {}
    }
}

/// No responseContext (visitorData, logged-in flags). An all-kinds page loses its section-level
/// song shelf ("Послушать ещё раз": the account's listening history) and its library tab.
fn fixture(mut resp: Value, all: bool) -> Value {
    if let Value::Object(o) = &mut resp {
        o.retain(|k, _| k == "contents" || k == "continuationContents");
    }
    if all
        && let Some(tabs) = resp
            .pointer_mut("/contents/tabbedSearchResultsRenderer/tabs")
            .and_then(Value::as_array_mut)
    {
        tabs.truncate(1);
        if let Some(sections) = tabs[0]
            .pointer_mut("/tabRenderer/content/sectionListRenderer/contents")
            .and_then(Value::as_array_mut)
        {
            let before = sections.len();
            sections.retain(|s| s.get("musicShelfRenderer").is_none());
            if sections.len() != before {
                println!("  (dropped {} history shelf)", before - sections.len());
            }
        }
    }
    scrub(&mut resp);
    resp
}

/// History suggestions are the account's own searches: out of the fixture.
fn suggest_fixture(mut resp: Value) -> Value {
    if let Value::Object(o) = &mut resp {
        o.retain(|k, _| k == "contents");
    }
    fn strip(v: &mut Value) {
        match v {
            Value::Array(a) => {
                a.retain(|i| i.get("historySuggestionRenderer").is_none());
                a.iter_mut().for_each(strip);
            }
            Value::Object(o) => o.values_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut resp);
    scrub(&mut resp);
    resp
}

fn names(a: &[mixtape_core::ArtistRef]) -> Vec<String> {
    a.iter()
        .map(|a| format!("{}{}", a.name, if a.id.is_some() { "" } else { "(no id)" }))
        .collect()
}

fn summary(label: &str, p: &SearchPage) {
    println!(
        "{label}: top={} tracks={} albums={} artists={} playlists={} continuation={}",
        match &p.top {
            Some(SearchItem::Track(t)) => format!("track {} {}", t.id, t.title),
            Some(SearchItem::Album(a)) => format!("album {} {}", a.id, a.title),
            Some(SearchItem::Artist(a)) => format!("artist {} {}", a.id, a.name),
            Some(SearchItem::Playlist(x)) => format!("playlist {} {}", x.id, x.title),
            None => "-".into(),
        },
        p.tracks.len(),
        p.albums.len(),
        p.artists.len(),
        p.playlists.len(),
        p.continuation.is_some()
    );
    for t in p.tracks.iter().take(3) {
        let album = t.album.as_ref().map(|a| a.title.as_str());
        println!(
            "  track {} {} — {:?} | {album:?} | {} ms",
            t.id,
            t.title,
            names(&t.artists),
            t.duration_ms
        );
    }
    for a in p.albums.iter().take(3) {
        println!(
            "  album {} {} — {:?} | {:?} {:?}",
            a.id,
            a.title,
            names(&a.artists),
            a.year,
            a.album_type
        );
    }
    for a in p.artists.iter().take(3) {
        println!(
            "  artist {} {} thumb={}",
            a.id,
            a.name,
            a.thumbnail.is_some()
        );
    }
    for x in p.playlists.iter().take(3) {
        println!(
            "  playlist {} {} — {:?} | {:?}",
            x.id, x.title, x.author, x.track_count
        );
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: search_probe <cookies.txt> [fixtures-dir]")?;
    let out = std::env::args().nth(2);
    let yt = InnerTube::new("ru", "RU")?;
    yt.set_cookies(Some(Cookies::parse(&std::fs::read_to_string(path)?)?));
    let save = |name: &str, v: Value| -> Result<(), Box<dyn std::error::Error>> {
        if let Some(dir) = &out {
            let text = serde_json::to_string(&v)?;
            println!("  -> {name}: {} KB", text.len() / 1024);
            std::fs::write(format!("{dir}/{name}"), text)?;
        }
        Ok(())
    };

    for (q, name) in [
        ("madeon", "search_all.json"),
        ("random access memories", "search_all_album.json"),
    ] {
        let raw = yt
            .call("search", search::search_body(q, SearchKind::All))
            .await?;
        let page = search::parse_search(&raw, SearchKind::All).ok_or("not a search page")?;
        summary(&format!("all {q:?}"), &page);
        save(name, fixture(raw, true))?;
    }
    for q in ["get lucky", "presenting daft punk"] {
        let page = yt.search(q, SearchKind::All).await?;
        summary(&format!("all {q:?}"), &page);
        println!("  top: {:?}", page.top);
    }

    let kinds = [
        (SearchKind::Tracks, "search_tracks.json"),
        (SearchKind::Albums, "search_albums.json"),
        (SearchKind::Artists, "search_artists.json"),
        (SearchKind::Playlists, "search_playlists.json"),
    ];
    for (kind, name) in kinds {
        let raw = yt
            .call("search", search::search_body("daft punk", kind))
            .await?;
        let page = search::parse_search(&raw, kind).ok_or("not a search page")?;
        summary(&format!("{kind:?}"), &page);
        save(name, fixture(raw, false))?;
        if let Some(token) = &page.continuation {
            let raw = yt.call("search", search::more_body(token)).await?;
            let more = search::parse_more(&raw, kind);
            summary(&format!("{kind:?} more"), &more);
            if kind == SearchKind::Tracks {
                save("search_tracks_more.json", fixture(raw, false))?;
            }
        }
    }

    let raw = yt
        .call(
            "music/get_search_suggestions",
            search::suggest_body("daft p"),
        )
        .await?;
    println!("suggest: {:?}", search::parse_suggestions(&raw));
    save("search_suggest.json", suggest_fixture(raw))?;
    println!("suggest via InnerTube: {:?}", yt.suggest("madeo").await?);

    // The public methods end to end: three pages of tracks.
    let mut page = yt.search("madeon", SearchKind::Tracks).await?;
    let mut total = page.tracks.len();
    for _ in 0..2 {
        let Some(token) = page.continuation.take() else {
            break;
        };
        page = yt.search_more(SearchKind::Tracks, &token).await?;
        total += page.tracks.len();
    }
    println!(
        "tracks over 3 pages of \"madeon\": {total}, more={}",
        page.continuation.is_some()
    );
    Ok(())
}
