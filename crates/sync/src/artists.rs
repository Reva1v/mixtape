//! Artist pictures: YouTube Music sends them only with the artist page, so they are
//! fetched separately — for one artist when its page opens, and for everyone without a
//! picture after a sync.

use mixtape_artwork::CoverStore;
use mixtape_core::ArtistId;
use mixtape_library::{Db, ytm};
use mixtape_yt::api::YtMusic;
use mixtape_yt::thumbs;
use std::sync::Arc;

const PARALLEL: usize = 3;

/// Fetches and stores one artist's picture under the artist id. Returns whether it did.
pub async fn fetch_image<Y: YtMusic>(yt: &Y, http: &reqwest::Client, db: &Db, covers: &CoverStore, id: &ArtistId) -> bool {
    let Ok(artist) = yt.artist(id).await else { return false };
    let Some(url) = artist.card.thumbnail else { return false };
    let Ok(bytes) = thumbs::download(http, &url).await else { return false };
    let (covers, key) = (covers.clone(), id.to_string());
    let processed = tokio::task::spawn_blocking(move || covers.process(&key, &bytes).ok()).await.ok().flatten();
    let Some(processed) = processed else { return false };
    let (id, key) = (id.clone(), id.to_string());
    db.write(move |t| ytm::set_artist_image(t, &id, &key, &processed.palette)).is_ok()
}

/// Fetches pictures for every credited artist that lacks one; returns how many arrived.
pub async fn fetch_missing<Y: YtMusic + 'static>(
    yt: Arc<Y>,
    http: reqwest::Client,
    db: Db,
    covers: CoverStore,
) -> usize {
    let Ok(ids) = db.read(ytm::artists_without_image) else { return 0 };
    let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL));
    let mut set = tokio::task::JoinSet::new();
    for id in ids {
        let (yt, http, db, covers, sem) = (yt.clone(), http.clone(), db.clone(), covers.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            fetch_image(&*yt, &http, &db, &covers, &id).await.then_some(())
        });
    }
    let mut n = 0;
    while let Some(r) = set.join_next().await {
        if matches!(r, Ok(Some(()))) {
            n += 1;
        }
    }
    n
}
