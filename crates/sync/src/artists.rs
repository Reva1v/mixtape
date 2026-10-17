//! Artist pictures: YouTube Music sends them only with the artist page, so they are
//! fetched separately — for one artist when its page opens, and for library artists
//! without a picture at startup and after a sync. Every look-up is recorded
//! (`image_checked_at`), so an artist without a picture is asked about once a week, not on
//! every start.

use mixtape_artwork::{CoverStore, Detail};
use mixtape_core::ArtistId;
use mixtape_library::repo::now_ms;
use mixtape_library::{Db, ytm};
use mixtape_yt::api::YtMusic;
use mixtape_yt::{YtError, thumbs};
use std::sync::Arc;

/// Artist pages read at once by [`fetch_missing`]; it runs in the background.
const PARALLEL: usize = 2;

/// Fetches and stores one artist's picture under the artist id, at `detail`. Returns
/// whether it did. The attempt is recorded unless the network or YouTube's throttling
/// kept it from being asked at all.
pub async fn fetch_image<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    id: &ArtistId,
    detail: Detail,
) -> bool {
    match fetch(yt, http, db, covers, id, detail).await {
        Ok(found) => {
            let checked = id.clone();
            let _ = db.write(move |t| ytm::artist_image_checked(t, &checked, now_ms()));
            found
        }
        Err(_) => false,
    }
}

/// `Err` when the question could not be asked (no network, throttled, signed out).
async fn fetch<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    id: &ArtistId,
    detail: Detail,
) -> Result<bool, YtError> {
    let transient = |e: YtError| match e {
        YtError::Network(_) | YtError::RateLimited | YtError::Auth => Err(e),
        _ => Ok(false),
    };
    let artist = match yt.artist(id).await {
        Ok(a) => a,
        Err(e) => return transient(e),
    };
    let Some(url) = artist.card.thumbnail else { return Ok(false) };
    let bytes = match detail {
        Detail::Full => thumbs::download(http, &url).await,
        Detail::Small => thumbs::download_small(http, &url).await,
    };
    let bytes = match bytes {
        Ok(b) => b,
        Err(e) => return transient(e),
    };
    let (covers, key) = (covers.clone(), id.to_string());
    let processed = tokio::task::spawn_blocking(move || covers.process(&key, &bytes, detail).ok()).await.ok().flatten();
    let Some(processed) = processed else { return Ok(false) };
    let (id, key) = (id.clone(), id.to_string());
    Ok(db.write(move |t| ytm::set_artist_image(t, &id, &key, &processed.palette)).is_ok())
}

/// Whether the artist's picture may be looked for now (not tried within a week).
pub fn image_due(db: &Db, id: &ArtistId) -> bool {
    db.read(|c| ytm::artist_image_due(c, id, now_ms() - ytm::ARTIST_IMAGE_RECHECK_MS)).unwrap_or(false)
}

/// Fetches pictures for library artists that lack one and were not looked for within a
/// week; returns how many arrived.
pub async fn fetch_missing<Y: YtMusic + 'static>(
    yt: Arc<Y>,
    http: reqwest::Client,
    db: Db,
    covers: CoverStore,
) -> usize {
    let Ok(ids) = db.read(|c| ytm::artists_without_image(c, now_ms() - ytm::ARTIST_IMAGE_RECHECK_MS)) else {
        return 0;
    };
    if ids.is_empty() {
        return 0;
    }
    tracing::debug!(n = ids.len(), "fetching artist pictures");
    let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL));
    let mut set = tokio::task::JoinSet::new();
    for id in ids {
        let (yt, http, db, covers, sem) = (yt.clone(), http.clone(), db.clone(), covers.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            fetch_image(&*yt, &http, &db, &covers, &id, Detail::Full).await.then_some(())
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
