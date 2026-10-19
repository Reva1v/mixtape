//! SoundCloud / Bandcamp import in a given data dir, without the app and without sound:
//! lists the link, downloads its tracks, prints the rows and probes every file. Running it
//! twice on the same dir shows that nothing is downloaded again.
//! `cargo run -p mixtape-sync --example external_e2e -- <yt-dlp.exe> <data dir> <url> [limit]`

use mixtape_artwork::CoverStore;
use mixtape_library::{DataPaths, Db, integrity, repo};
use mixtape_sync::external::{self, Target};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let ytdlp = PathBuf::from(&args[1]);
    let paths = DataPaths::at(PathBuf::from(&args[2]))?;
    let db = Db::open(&paths.db)?;
    let covers = CoverStore::new(&paths.covers);
    let http = reqwest::Client::new();
    let link = mixtape_sync::url_import::parse_external(&args[3])
        .ok_or("not a SoundCloud / Bandcamp link")?;
    let started = std::time::Instant::now();
    let mut listing = external::list(&ytdlp, &link).await?;
    if let Some(limit) = args.get(4).and_then(|s| s.parse().ok()) {
        listing.entries.truncate(limit);
    }
    println!(
        "listed in {:?}: {} «{}», {} tracks, set album {:?}",
        started.elapsed(),
        listing.kind_name(),
        listing.title,
        listing.entries.len(),
        listing.set_album
    );
    let target = Target {
        ytdlp: &ytdlp,
        root: &paths.root,
        http: &http,
        db: &db,
        covers: &covers,
    };
    let report = external::import(&target, &listing, |done, total| {
        println!("  {done}/{total}")
    })
    .await;
    println!("report in {:?}: {report:?}", started.elapsed());
    for e in &listing.entries {
        let Some(t) = db.read(|c| repo::track(c, &e.id))? else {
            continue;
        };
        let file = db
            .read(|c| repo::media_file(c, &e.id))?
            .ok_or("no media row")?;
        let probe = mixtape_audio::probe(&paths.root.join(&file.path))?;
        println!(
            "{} | {} — {} | album {:?} #{:?} | {} ms | {:?} | {} {} {} Hz {} ms | cover {:?}",
            t.id,
            t.artist_names(),
            t.title,
            t.album.map(|a| a.title),
            t.track_no,
            t.duration_ms,
            t.state,
            file.path,
            probe.codec,
            probe.sample_rate,
            probe.duration_ms,
            t.cover_key.map(|k| covers.exists(&k))
        );
    }
    println!("integrity: {:?}", integrity::check(&db, &paths.root)?);
    Ok(())
}
