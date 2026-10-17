//! Live check of the home feed:
//! `cargo run -p mixtape-yt --example home_probe -- <cookies.txt> [fixtures-dir]`.
//! Prints shelf titles and item kinds; never prints cookie values or account data. With
//! `fixtures-dir`, writes the first page, the first continuation and a page with song rows
//! there (minified), trimmed to what the parser reads and anonymized: every title, name and
//! id is synthetic, so no listening history leaves the machine. Read-only requests only.
//! `home_probe --anonymize <fixture.json>...` re-applies the same anonymization offline.

use mixtape_yt::auth::Cookies;
use mixtape_yt::home::{self, HomeItem, HomeShelf};
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::parse::{find_all, text};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Keys that are noise for the parser or tied to the session/account.
const DROP: &[&str] = &[
    "menu",
    "trackingParams",
    "clickTrackingParams",
    "accessibility",
    "accessibilityData",
    "loggingContext",
    "feedbackEndpoint",
    "feedbackToken",
    "serializedShareEntity",
    "strapline",
    "moreContentButton",
    "overlay",
    "thumbnailOverlay",
    "playerParams",
    "params",
    "badges",
    "customIndexColumn",
    "itemHeight",
    "flexColumnDisplayStyle",
    "displayPriority",
    "aspectRatio",
    "thumbnailCrop",
    "thumbnailScale",
    "itemThumbnailOverlay",
    "subtitleBadges",
    "loggingDirectives",
];

/// Strings that identify the account: its name, handle, email and channel id.
fn account_strings(menu: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["accountName", "channelHandle", "email"] {
        for v in find_all(menu, key) {
            let s = text(Some(v));
            let s = if s.is_empty() {
                v.as_str().unwrap_or("").to_owned()
            } else {
                s
            };
            if s.len() >= 3 {
                out.push(s);
            }
        }
    }
    for id in find_all(menu, "browseId") {
        if let Some(id) = id.as_str().filter(|i| i.starts_with("UC")) {
            out.push(id.to_owned());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn scrub(v: &mut Value, secrets: &[String]) {
    match v {
        Value::Object(o) => {
            o.retain(|k, _| !DROP.contains(&k.as_str()));
            // Personal shelves ("Послушать ещё раз") show the account's avatar in the header.
            if let Some(Value::Object(h)) = o.get_mut("musicCarouselShelfBasicHeaderRenderer") {
                h.remove("thumbnail");
            }
            for (k, child) in o.iter_mut() {
                if (k == "continuation" || k == "token") && child.is_string() {
                    *child = Value::String(format!("REDACTED_{}", k.to_uppercase()));
                } else {
                    scrub(child, secrets);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|c| scrub(c, secrets)),
        Value::String(s) => {
            for secret in secrets {
                if s.contains(secret.as_str()) {
                    *s = s.replace(secret.as_str(), "User");
                }
            }
        }
        _ => {}
    }
}

/// Only the feed survives: no responseContext (visitorData, logged-in flags), header or
/// frameworkUpdates. Then every title, name and id is replaced with a synthetic one.
fn fixture(mut resp: Value, secrets: &[String], anon: &mut Anon) -> Value {
    if let Value::Object(o) = &mut resp {
        o.retain(|k, _| k == "contents" || k == "continuationContents");
    }
    scrub(&mut resp, secrets);
    anon.walk(&mut resp, &mut Vec::new());
    resp
}

const THUMB: &str = "https://lh3.googleusercontent.com/synthetic=w120-h120";

/// Id prefixes the parser looks at; the rest of the id becomes synthetic.
const ID_PREFIXES: &[&str] = &[
    "RDCLAK5uy_",
    "RDTMAK5uy_",
    "OLAK5uy_",
    "MPREb_",
    "MPLA",
    "UC",
    "PL",
    "RD",
];

/// Words the parser needs (type labels, play counts) stay; the rest is personal taste.
const LABELS: &[&str] = &[
    "Композиция",
    "Альбом",
    "Сингл",
    "EP",
    "Плейлист",
    "Видео",
    "Исполнитель",
    "Микс",
    "Создан автоматически",
];
const COUNT_WORDS: &[&str] = &["прослушиван", "просмотр", "подписчик", "трек"];

/// `n` if `s` is `prefix` followed by `width` digits (or any digits when `width` is 0):
/// a value this anonymizer produced before.
fn synthetic(s: &str, prefix: &str, width: usize) -> Option<usize> {
    let digits = s.strip_prefix(prefix)?;
    let ok = !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && (width == 0 || digits.len() == width);
    ok.then(|| digits.parse().ok()).flatten()
}

/// Deterministic anonymizer: the same original maps to the same synthetic value across all
/// files written in one run (videoIds stay 11 characters, id prefixes are kept).
#[derive(Default)]
struct Anon {
    videos: HashMap<String, String>,
    ids: HashMap<String, String>,
    texts: HashMap<(&'static str, String), String>,
    counters: HashMap<&'static str, usize>,
    originals: HashSet<String>,
}

impl Anon {
    fn next(&mut self, kind: &'static str) -> usize {
        let n = self.counters.entry(kind).or_insert(0);
        *n += 1;
        *n
    }

    /// Keeps a synthetic value as it is and moves the counter past it.
    fn seen(&mut self, kind: &'static str, n: usize, s: &str) -> String {
        let c = self.counters.entry(kind).or_insert(0);
        *c = (*c).max(n);
        s.to_owned()
    }

    fn video(&mut self, id: &str) -> String {
        if let Some(n) = synthetic(id, "v", 10) {
            return self.seen("video", n, id);
        }
        if let Some(v) = self.videos.get(id) {
            return v.clone();
        }
        let v = format!("v{:010}", self.next("video"));
        self.originals.insert(id.to_owned());
        self.videos.insert(id.to_owned(), v.clone());
        v
    }

    fn id(&mut self, id: &str) -> String {
        if id.starts_with("FE") || matches!(id, "LM" | "VLLM") {
            return id.to_owned();
        }
        if let Some(rest) = id.strip_prefix("VL") {
            return format!("VL{}", self.id(rest));
        }
        if let Some(video) = id.strip_prefix("RDAMVM") {
            return format!("RDAMVM{}", self.video(video));
        }
        let prefix = ID_PREFIXES
            .iter()
            .find(|p| id.starts_with(**p))
            .copied()
            .unwrap_or("");
        if let Some(n) = synthetic(id, &format!("{prefix}syn"), 5) {
            return self.seen("id", n, id);
        }
        if let Some(v) = self.ids.get(id) {
            return v.clone();
        }
        let v = format!("{prefix}syn{:05}", self.next("id"));
        self.originals.insert(id.to_owned());
        self.ids.insert(id.to_owned(), v.clone());
        v
    }

    fn text(&mut self, kind: &'static str, s: &str) -> String {
        let t = s.trim();
        // Already synthetic (a fixture anonymized before) and of the right kind: keep it.
        // Shelf titles are always renumbered, they may carry another kind from an older run.
        if kind != "Shelf"
            && let Some(n) = synthetic(t, &format!("{kind} "), 0)
        {
            return self.seen(kind, n, s);
        }
        let keep = t.chars().all(|c| !c.is_alphanumeric())
            || matches!(t, "и" | "and" | "x" | "feat." | "ft.")
            || LABELS.contains(&t)
            || (t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()));
        if keep {
            return s.to_owned();
        }
        // "836 млн прослушиваний": the unit stays, the exact number (a fingerprint) does not.
        if t.chars().any(|c| c.is_ascii_digit()) && COUNT_WORDS.iter().any(|w| t.contains(w)) {
            let unit = t.trim_start_matches(|c: char| {
                c.is_ascii_digit() || matches!(c, ',' | '.' | ' ' | '\u{a0}' | '\u{202f}')
            });
            return format!("12\u{a0}{unit}");
        }
        let key = (kind, s.to_owned());
        if let Some(v) = self.texts.get(&key) {
            return v.clone();
        }
        let v = format!("{kind} {}", self.next(kind));
        self.texts.insert(key, v.clone());
        v
    }

    /// What a run of text names, judging by its link and where it sits.
    fn kind(o: &serde_json::Map<String, Value>, path: &[String]) -> &'static str {
        let inside = |k: &str| path.iter().any(|p| p == k);
        if inside("musicCarouselShelfBasicHeaderRenderer") {
            return "Shelf";
        }
        if let Some(nav) = o.get("navigationEndpoint") {
            let page = mixtape_yt::parse::find_first(nav, "pageType").and_then(Value::as_str);
            return match page {
                Some("MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL") => "Artist",
                Some("MUSIC_PAGE_TYPE_ALBUM") => "Album",
                Some("MUSIC_PAGE_TYPE_PLAYLIST") => "Playlist",
                _ if nav.get("watchEndpoint").is_some() => "Track",
                _ => "Link",
            };
        }
        if inside("chipCloudChipRenderer") {
            "Chip"
        } else if path.iter().rev().nth(2).is_some_and(|p| p == "title") {
            "Title"
        } else {
            "Text"
        }
    }

    fn walk(&mut self, v: &mut Value, path: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                let kind = Self::kind(o, path);
                for (k, child) in o.iter_mut() {
                    if let Value::String(s) = child {
                        match k.as_str() {
                            "videoId" => *s = self.video(s),
                            "browseId" | "playlistId" => *s = self.id(s),
                            "url" if s.starts_with("http") || s.starts_with("//") => {
                                *s = THUMB.to_owned()
                            }
                            "url" => *s = "/synthetic".to_owned(),
                            "text" | "simpleText" => *s = self.text(kind, s),
                            "shelfId" if !s.starts_with("shelf-") => {
                                *s = format!("shelf-{}", self.next("shelf"))
                            }
                            _ => {}
                        }
                        continue;
                    }
                    path.push(k.clone());
                    self.walk(child, path);
                    path.pop();
                }
            }
            Value::Array(a) => {
                for c in a {
                    path.push(String::new());
                    self.walk(c, path);
                    path.pop();
                }
            }
            _ => {}
        }
    }

    /// Ids of the original response still present in the output.
    fn leaked(&self, out: &str) -> usize {
        self.originals
            .iter()
            .filter(|id| id.len() >= 6 && out.contains(id.as_str()))
            .count()
    }
}

fn kind(item: &HomeItem) -> &'static str {
    match item {
        HomeItem::Track(t) if t.kind == mixtape_yt::api::VideoKind::Audio => "track",
        HomeItem::Track(_) => "video",
        HomeItem::Album(_) => "album",
        HomeItem::Playlist(_) => "playlist",
        HomeItem::Artist(_) => "artist",
        HomeItem::Mix { .. } => "mix",
    }
}

fn summary(shelves: &[HomeShelf]) {
    for s in shelves {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for i in &s.items {
            match counts.iter_mut().find(|(k, _)| *k == kind(i)) {
                Some((_, n)) => *n += 1,
                None => counts.push((kind(i), 1)),
            }
        }
        println!("  «{}»: {counts:?}", s.title);
    }
}

/// Scrubs, anonymizes and checks one response, then writes it minified to `path`.
fn write_fixture(
    path: &str,
    resp: Value,
    secrets: &[String],
    anon: &mut Anon,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = serde_json::to_string(&fixture(resp, secrets, anon))?;
    if secrets.iter().any(|s| text.contains(s.as_str())) {
        return Err(format!("{path}: account data survived the scrub").into());
    }
    if anon.leaked(&text) > 0 {
        return Err(format!("{path}: {} original ids survived", anon.leaked(&text)).into());
    }
    println!("  -> {path}: {} KB", text.len() / 1024);
    std::fs::write(path, text)?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let usage =
        "usage: home_probe <cookies.txt> [fixtures-dir] | home_probe --anonymize <fixture.json>...";
    let path = std::env::args().nth(1).ok_or(usage)?;
    // Offline: re-anonymize saved fixtures in place, with one mapping across all files.
    if path == "--anonymize" {
        let mut anon = Anon::default();
        for file in std::env::args().skip(2) {
            let resp: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
            write_fixture(&file, resp, &[], &mut anon)?;
        }
        return Ok(());
    }
    let out = std::env::args().nth(2);
    let yt = InnerTube::new("ru", "RU")?;
    yt.set_cookies(Some(Cookies::parse(&std::fs::read_to_string(path)?)?));

    let secrets = account_strings(
        &yt.call("account/account_menu", serde_json::json!({}))
            .await?,
    );
    println!("account strings to redact: {}", secrets.len());
    let mut anon = Anon::default();
    let mut save = |name: &str, resp: Value| -> Result<(), Box<dyn std::error::Error>> {
        match &out {
            Some(dir) => write_fixture(&format!("{dir}/{name}"), resp, &secrets, &mut anon),
            None => Ok(()),
        }
    };

    // Page by page: the first page, the first continuation and the first page with mixes
    // (cards that open a radio queue rather than a song) become fixtures.
    let mut raw = yt.browse("FEmusic_home").await?;
    let (mut saved_mixes, mut saved_rows) = (false, false);
    for n in 0..10 {
        let page = home::parse_page(&raw);
        let cards = find_all(&raw, "musicTwoRowItemRenderer").len()
            + find_all(&raw, "musicResponsiveListItemRenderer").len();
        let parsed: usize = page.shelves.iter().map(|s| s.items.len()).sum();
        println!(
            "page {n}: {} shelves, {parsed}/{cards} items parsed, continuation={}",
            page.shelves.len(),
            page.continuation.is_some()
        );
        summary(&page.shelves);
        let has_mix = page
            .shelves
            .iter()
            .flat_map(|s| &s.items)
            .any(|i| matches!(i, HomeItem::Mix { .. }));
        let has_rows = !find_all(&raw, "musicResponsiveListItemRenderer").is_empty();
        let name = match n {
            0 => Some("browse_home.json"),
            1 => Some("browse_home_more.json"),
            _ if has_rows && !saved_rows => Some("browse_home_rows.json"),
            _ if has_mix && !saved_mixes => Some("browse_home_mixes.json"),
            _ => None,
        };
        if let Some(name) = name {
            saved_rows |= has_rows;
            saved_mixes |= has_mix;
            save(name, raw.clone())?;
        }
        let Some(token) = page.continuation else {
            break;
        };
        raw = yt.browse_continuation(&token).await?;
    }

    // The public method end to end.
    let all = yt.home(30).await?;
    println!("InnerTube::home(30): {} shelves", all.len());
    summary(&all);
    for s in all.iter().take(4) {
        for i in s.items.iter().take(2) {
            match i {
                HomeItem::Track(t) => println!("    track {} {} {:?}", t.id, t.title, t.kind),
                HomeItem::Album(a) => println!("    album {} {}", a.id, a.title),
                HomeItem::Playlist(p) => println!("    playlist {} {}", p.id, p.title),
                HomeItem::Artist(a) => println!("    artist {} {}", a.id, a.name),
                HomeItem::Mix {
                    playlist_id,
                    title,
                    seed,
                    ..
                } => {
                    println!("    mix {playlist_id} {title} seed={seed:?}")
                }
            }
        }
    }
    Ok(())
}
