//! Live check of the mix path without playing anything: radio for a seed, store the page,
//! resolve the next tracks, stream one into a sparse cache file, finalize, probe, measure.
//! `cargo run -p mixtape-sync --example mix_e2e -- <cookies.txt> <sidecars dir> <video id>`

use mixtape_artwork::CoverStore;
use mixtape_audio::SparseCacheFile;
use mixtape_core::TrackId;
use mixtape_library::{DataPaths, Db, repo};
use mixtape_sync::stream::StreamFill;
use mixtape_yt::auth::Cookies;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::resolver::StreamResolver;
use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&args[1])?;
    let bins = PathBuf::from(&args[2]);
    let seed = TrackId(args[3].clone());
    let dir = std::env::temp_dir().join(format!("mixtape-mix-e2e-{}", std::process::id()));
    let paths = DataPaths::at(&dir)?;
    let db = Db::open(&paths.db)?;
    let covers = CoverStore::new(&paths.covers);
    let yt = InnerTube::new("ru", "RU")?;
    let cookies = Cookies::parse(&text)?;
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp)?;
    let resolver = StreamResolver::new(
        bins.join("yt-dlp-x86_64-pc-windows-msvc.exe"),
        bins.join("deno-x86_64-pc-windows-msvc.exe"),
        tmp,
    );
    resolver.set_cookies(Some(cookies.to_netscape()));
    yt.set_cookies(Some(cookies));

    let t = Instant::now();
    let page = yt.radio(&seed).await?;
    println!(
        "radio: {} tracks, {} chips, continuation {} ({:?})",
        page.tracks.len(),
        page.chips.len(),
        page.continuation.is_some(),
        t.elapsed()
    );
    let ids = mixtape_sync::sources::store_tracks(yt.http(), &db, &covers, page.tracks).await?;
    let stored = db.read(|c| repo::tracks_by_ids(c, &ids))?;
    let with_cover = stored.iter().filter(|t| t.cover_key.is_some()).count();
    println!("stored {} (covers {with_cover}), states {:?}", stored.len(), stored.first().map(|t| t.state));

    let next: Vec<TrackId> = ids.iter().skip(1).take(5).cloned().collect();
    let t = Instant::now();
    let resolved = resolver.resolve(&next).await;
    println!(
        "resolved {}/{} in {:?}",
        resolved.iter().filter(|(_, r)| r.is_ok()).count(),
        next.len(),
        t.elapsed()
    );
    let (id, stream) = resolved.into_iter().find_map(|(id, r)| r.ok().map(|s| (id, s))).ok_or("nothing resolved")?;
    let len = mixtape_sync::fetch::exact_len(yt.http(), &stream.url).await?;
    let part = paths.root.join("cache").join(format!("{id}.{}.part", stream.container.as_str()));
    let file = SparseCacheFile::create(&part, len)?;
    let t = Instant::now();
    let fill = StreamFill::start(yt.http().clone(), stream.url.clone(), file.clone(), || println!("expired"));
    // A reader that seeks to the middle first, like a user scrubbing.
    let mut reader = file.reader();
    {
        use std::io::{Read, Seek, SeekFrom};
        reader.seek(SeekFrom::Start(len / 2))?;
        let mut buf = [0u8; 4096];
        let n = tokio::task::spawn_blocking(move || reader.read(&mut buf)).await??;
        println!("mid read {n} B after {:?}", t.elapsed());
    }
    fill.wait().await?;
    println!("filled {len} B ({}) in {:?}", stream.container.as_str(), t.elapsed());
    let dest = paths.root.join("cache").join(format!("{id}.{}", stream.container.as_str()));
    file.finalize(&dest)?;
    let probe = mixtape_audio::probe(&dest)?;
    let loud = mixtape_audio::measure_loudness(&dest)?;
    println!(
        "probe: {} Hz, {} ms; loudness {:.1} LUFS, peak {:.2}",
        probe.sample_rate, probe.duration_ms, loud.lufs, loud.true_peak
    );
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}
