//! End-to-end check in a temporary data dir: import one link, download it, print the row.
//! `cargo run -p mixtape-sync --example e2e -- <cookies.txt> <sidecars dir> <url>`

use mixtape_artwork::CoverStore;
use mixtape_library::{DataPaths, Db, jobs, repo};
use mixtape_sync::downloader::{Analysis, DownloadsProgress, Downloader, Hooks};
use mixtape_yt::auth::Cookies;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::resolver::StreamResolver;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Print;
impl Hooks for Print {
    fn analyze(&self, _: &Path) -> Analysis {
        Analysis::default()
    }
    fn progress(&self, p: &DownloadsProgress) {
        println!("progress: {}/{} failed {} {:?} {} B/s", p.done, p.total, p.failed, p.current.as_ref().map(|c| (c.bytes_done, c.bytes_total)), p.bytes_per_sec);
    }
    fn finished(&self, t: &mixtape_core::TrackId) {
        println!("finished {t}");
    }
    fn auth_expired(&self) {
        println!("auth expired");
    }
    fn low_space(&self) {
        println!("low space");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&args[1])?;
    let bins = PathBuf::from(&args[2]);
    let dir = std::env::temp_dir().join(format!("mixtape-e2e-{}", std::process::id()));
    let paths = DataPaths::at(&dir)?;
    let db = Db::open(&paths.db)?;
    let covers = CoverStore::new(&paths.covers);
    let yt = InnerTube::new("ru", "RU")?;
    let cookies = Cookies::parse(&text)?;
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp)?;
    let resolver = Arc::new(StreamResolver::new(bins.join("yt-dlp-x86_64-pc-windows-msvc.exe"), bins.join("deno-x86_64-pc-windows-msvc.exe"), tmp));
    resolver.set_cookies(Some(cookies.to_netscape()));
    yt.set_cookies(Some(cookies));
    let link = mixtape_sync::url_import::parse_link(&args[3]).ok_or("bad link")?;
    let r = mixtape_sync::sources::import_link(&yt, yt.http(), &db, &covers, link).await?;
    println!("imported {:?}", r);
    let dl = Downloader::start(&tokio::runtime::Handle::current(), db.clone(), resolver, yt.http().clone(), paths.root.clone(), Arc::new(Print));
    dl.wake();
    for _ in 0..120 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let s = db.read(jobs::summary)?;
        if s.pending() == 0 {
            break;
        }
    }
    println!("summary {:?}", db.read(jobs::summary)?);
    println!("failed {:?}", db.read(jobs::failed_list)?);
    let ids: Vec<_> = db.read(|c| Ok(c.prepare("SELECT id FROM tracks")?.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?))?;
    for id in ids {
        let t = db.read({ let id = mixtape_core::TrackId::new(&id); move |c| repo::track(c, &id) })?;
        let m = db.read({ let id = mixtape_core::TrackId::new(&id); move |c| repo::media_file(c, &id) })?;
        println!("{:?} | {:?}", t.map(|t| (t.title, t.state, t.cover_key)), m.map(|m| m.path));
    }
    for e in std::fs::read_dir(paths.library)? {
        let e = e?;
        println!("file {} {} bytes", e.file_name().to_string_lossy(), e.metadata()?.len());
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
