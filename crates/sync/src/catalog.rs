//! The YouTube Music catalog outside the library (US4, T082): albums, artists and playlists
//! opened from search or the home feed are stored as `remote` metadata so they can be shown
//! and played; «Скачать» queues their downloads.

use crate::sources::{self, Collected};
use mixtape_artwork::{CoverStore, Detail};
use mixtape_core::{AlbumId, AlbumType, ArtistId, Playlist, PlaylistId, PlaylistOrigin, TrackId};
use mixtape_library::repo::{self, AlbumMeta};
use mixtape_library::{Db, ytm};
use mixtape_yt::YtError;
use mixtape_yt::api::{YtAlbumCard, YtArtistCard, YtMusic, YtPlaylistCard};
use mixtape_yt::thumbs;
use std::collections::HashSet;
use std::sync::Arc;

fn lib_err(e: mixtape_library::Error) -> YtError {
    YtError::Parse(format!("библиотека: {e}"))
}

/// Stores a full album page; `download` also queues its tracks. Returns the track count.
pub async fn store_album<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    id: &AlbumId,
    download: bool,
) -> Result<usize, YtError> {
    let album = yt.album(id).await?;
    let n = album.tracks.len();
    let mut c = Collected::default();
    for t in &album.tracks {
        c.sources.insert(t.id.clone(), "catalog".into());
    }
    c.tracks.extend(album.tracks.iter().cloned());
    c.albums.push(album);
    sources::store(http, db, covers, c, download).await.map_err(lib_err)?;
    Ok(n)
}

/// What an artist has on YouTube Music: popular tracks (stored as `remote`) and the
/// discography (stored as album cards).
pub struct ArtistCatalog {
    pub top: Vec<TrackId>,
    pub albums: Vec<AlbumId>,
}

pub async fn store_artist<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    id: &ArtistId,
) -> Result<ArtistCatalog, YtError> {
    let artist = yt.artist(id).await?;
    let top = sources::store_tracks(http, db, covers, artist.top_tracks).await.map_err(lib_err)?;
    store_artist_cards(http, db, covers, &[artist.card]).await;
    let albums = store_album_cards(http, db, covers, &artist.discography).await;
    Ok(ArtistCatalog { top, albums })
}

/// Queues the whole discography; returns how many tracks were queued or already owned.
pub async fn download_artist<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    id: &ArtistId,
) -> Result<usize, YtError> {
    let artist = yt.artist(id).await?;
    let mut total = 0;
    for card in &artist.discography {
        match store_album(yt, http, db, covers, &card.id, true).await {
            Ok(n) => total += n,
            Err(e) => tracing::warn!(album = %card.id, error = %e, "discography album failed"),
        }
    }
    Ok(total)
}

/// A playlist from the catalog, its tracks stored as `remote`; it is not added to the
/// library's playlists.
pub async fn playlist_page<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    id: &PlaylistId,
) -> Result<(Playlist, Vec<TrackId>), YtError> {
    let p = yt.playlist(id).await?;
    let tracks = sources::store_tracks(http, db, covers, p.tracks).await.map_err(lib_err)?;
    let cover_key = cover(http, covers, id.as_str(), p.card.thumbnail.as_deref()).await;
    let playlist = Playlist {
        id: p.card.id,
        title: p.card.title,
        origin: PlaylistOrigin::Ytm,
        track_count: tracks.len() as u32,
        cover_key,
    };
    Ok((playlist, tracks))
}

/// Downloads a missing card cover (small: these are only browsed); returns its key when
/// it is on disk.
async fn cover(http: &reqwest::Client, covers: &CoverStore, key: &str, url: Option<&str>) -> Option<String> {
    if !covers.exists(key)
        && let Some(url) = url
        && let Ok(bytes) = thumbs::download_small(http, url).await
    {
        let (covers, key2) = (covers.clone(), key.to_owned());
        let _ = tokio::task::spawn_blocking(move || covers.process(&key2, &bytes, Detail::Small)).await;
    }
    covers.exists(key).then(|| key.to_owned())
}

/// Fetches missing covers six at a time; returns the keys that are on disk afterwards.
async fn covers_for(http: &reqwest::Client, covers: &CoverStore, jobs: Vec<(String, Option<String>)>) -> HashSet<String> {
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set = tokio::task::JoinSet::new();
    for (key, url) in jobs {
        let (http, covers, sem) = (http.clone(), covers.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            cover(&http, &covers, &key, url.as_deref()).await
        });
    }
    let mut have = HashSet::new();
    while let Some(res) = set.join_next().await {
        if let Ok(Some(key)) = res {
            have.insert(key);
        }
    }
    have
}

/// Stores album cards (search results, a discography) that are not known yet.
pub async fn store_album_cards(http: &reqwest::Client, db: &Db, covers: &CoverStore, cards: &[YtAlbumCard]) -> Vec<AlbumId> {
    let have = covers_for(http, covers, cards.iter().map(|c| (c.id.to_string(), c.thumbnail.clone())).collect()).await;
    let mut metas = Vec::new();
    for c in cards {
        let key = c.id.to_string();
        let cover_key = have.contains(&key).then_some(key);
        let palette = cover_key.as_ref().and_then(|k| covers.palette(k).ok()).unwrap_or_default();
        metas.push(AlbumMeta {
            id: c.id.clone(),
            title: c.title.clone(),
            artists: c.artists.clone(),
            year: c.year,
            album_type: c.album_type.unwrap_or(AlbumType::Album),
            cover_key,
            palette,
            track_count: None,
        });
    }
    let ids = metas.iter().map(|m| m.id.clone()).collect();
    let res = db.write(move |t| {
        for m in &metas {
            // A known album keeps what its full page said; only a missing cover is filled in.
            if ytm::album_exists(t, &m.id)? {
                if let Some(key) = &m.cover_key {
                    t.execute(
                        "UPDATE albums SET cover_key = COALESCE(cover_key, ?2) WHERE id = ?1",
                        rusqlite::params![m.id.as_str(), key],
                    )?;
                }
            } else {
                repo::upsert_album(t, m)?;
            }
        }
        Ok(())
    });
    if let Err(e) = res {
        tracing::warn!(error = %e, "storing album cards failed");
    }
    ids
}

/// Stores artist cards with their pictures (only where none is stored yet).
pub async fn store_artist_cards(http: &reqwest::Client, db: &Db, covers: &CoverStore, cards: &[YtArtistCard]) -> Vec<ArtistId> {
    let have = covers_for(http, covers, cards.iter().map(|c| (c.id.to_string(), c.thumbnail.clone())).collect()).await;
    let mut rows = Vec::new();
    for c in cards {
        let key = c.id.to_string();
        let image = have.contains(&key).then_some(key);
        let palette = image.as_ref().and_then(|k| covers.palette(k).ok()).unwrap_or_default();
        rows.push((c.clone(), image, palette));
    }
    let ids = rows.iter().map(|(c, _, _)| c.id.clone()).collect();
    let res = db.write(move |t| {
        for (c, image, palette) in &rows {
            repo::upsert_artist(
                t,
                &mixtape_core::Artist {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    image_key: None,
                    in_library: false,
                    palette: Vec::new(),
                },
            )?;
            if let Some(key) = image
                && repo::artist(t, &c.id)?.is_some_and(|a| a.image_key.is_none())
            {
                ytm::set_artist_image(t, &c.id, key, palette)?;
            }
        }
        Ok(())
    });
    if let Err(e) = res {
        tracing::warn!(error = %e, "storing artist cards failed");
    }
    ids
}

/// Playlist cards from search, with covers, as the UI's `Playlist`.
pub async fn playlist_cards(http: &reqwest::Client, covers: &CoverStore, cards: Vec<YtPlaylistCard>) -> Vec<Playlist> {
    let have = covers_for(http, covers, cards.iter().map(|c| (c.id.to_string(), c.thumbnail.clone())).collect()).await;
    let mut out = Vec::with_capacity(cards.len());
    for c in cards {
        let cover_key = have.contains(c.id.as_str()).then(|| c.id.to_string());
        out.push(Playlist {
            id: c.id,
            title: c.title,
            origin: PlaylistOrigin::Ytm,
            track_count: c.track_count.unwrap_or(0),
            cover_key,
        });
    }
    out
}
