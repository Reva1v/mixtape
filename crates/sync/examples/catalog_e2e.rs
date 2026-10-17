//! Live check of the catalog path in a temporary data dir, nothing played or downloaded:
//! search, store the cards, open an album, an artist and a playlist from the results.
//! `cargo run -p mixtape-sync --example catalog_e2e -- <cookies.txt> <query>`

use mixtape_artwork::CoverStore;
use mixtape_library::{DataPaths, Db, repo};
use mixtape_sync::{catalog, sources};
use mixtape_yt::auth::Cookies;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::search::SearchKind;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::env::temp_dir().join(format!("mixtape-catalog-e2e-{}", std::process::id()));
    let paths = DataPaths::at(&dir)?;
    let db = Db::open(&paths.db)?;
    let covers = CoverStore::new(&paths.covers);
    let yt = InnerTube::new("ru", "RU")?;
    yt.set_cookies(Some(Cookies::parse(&std::fs::read_to_string(&args[1])?)?));
    let http = yt.http();

    let t = Instant::now();
    let page = yt.search(&args[2], SearchKind::All).await?;
    println!("search: top {:?}, {} tracks, {} albums, {} artists, {} playlists ({:?})",
        page.top.as_ref().map(|_| "yes"), page.tracks.len(), page.albums.len(), page.artists.len(), page.playlists.len(), t.elapsed());
    let t = Instant::now();
    let ids = sources::store_tracks(http, &db, &covers, page.tracks.clone()).await?;
    let albums = catalog::store_album_cards(http, &db, &covers, &page.albums).await;
    let artists = catalog::store_artist_cards(http, &db, &covers, &page.artists).await;
    let stored = db.read(|c| repo::tracks_by_ids(c, &ids))?;
    let al = db.read(|c| repo::albums_by_ids(c, &albums))?;
    let ar = db.read(|c| repo::artists_by_ids(c, &artists))?;
    println!("stored {} tracks, {} albums ({} with cover), {} artists ({} with picture) in {:?}",
        stored.len(), al.len(), al.iter().filter(|a| a.cover_key.is_some()).count(),
        ar.len(), ar.iter().filter(|a| a.image_key.is_some()).count(), t.elapsed());

    if let Some(a) = albums.first() {
        let n = catalog::store_album(&yt, http, &db, &covers, a, false).await?;
        let album = db.read(|c| repo::album(c, a))?.unwrap();
        let tracks = db.read(|c| repo::album_track_ids(c, a))?;
        println!("album {:?}: {n} tracks from page, {} stored, track_count {:?}", album.title, tracks.len(), album.track_count);
        let queued = db.read(mixtape_library::jobs::summary)?;
        println!("  jobs after a view-only open: {queued:?}");
    }
    if let Some(a) = artists.first() {
        let t = Instant::now();
        let found = catalog::store_artist(&yt, http, &db, &covers, a).await?;
        println!("artist {a}: top {}, discography {} ({:?})", found.top.len(), found.albums.len(), t.elapsed());
    }
    if let Some(p) = page.playlists.first() {
        let (pl, tracks) = catalog::playlist_page(&yt, http, &db, &covers, &p.id).await?;
        let in_library = db.read(|c| repo::playlist(c, &p.id))?.is_some();
        println!("playlist {:?}: {} tracks, cover {}, added to library: {in_library}", pl.title, tracks.len(), pl.cover_key.is_some());
    }
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}
