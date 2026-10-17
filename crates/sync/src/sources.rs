//! Source sync (T060): reads the chosen YouTube Music sources, writes their metadata to
//! the library, fetches covers and queues downloads for tracks not yet in the library.

use mixtape_artwork::CoverStore;
use mixtape_core::{AlbumId, AlbumRef, AlbumType, Artist, PlaylistId, TrackId};
use mixtape_library::repo::{self, AlbumMeta, TrackMeta};
use mixtape_library::{Db, jobs, ytm};
use mixtape_yt::api::{YtAlbum, YtMusic, YtTrack};
use mixtape_yt::{YtError, thumbs};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// What the user chose on the sync screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPlan {
    pub liked: bool,
    pub playlist_ids: Vec<PlaylistId>,
    /// Albums saved in the library.
    pub albums: bool,
    /// Full discographies of subscribed artists.
    pub artists: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Tracks seen across all sources.
    pub tracks: usize,
    /// New download jobs.
    pub queued: usize,
    /// Sources that failed, with a Russian reason.
    pub errors: Vec<(String, String)>,
}

/// Progress for the UI: which source is being read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStage {
    Liked,
    Playlist(String),
    Albums { done: usize, total: usize },
    Artists { done: usize, total: usize },
    Covers { done: usize, total: usize },
    Saving,
}

const COVER_PARALLEL: usize = 6;

/// One source's tracks, tagged for `download_jobs.source`.
#[derive(Default)]
pub(crate) struct Collected {
    pub(crate) tracks: Vec<YtTrack>,
    pub(crate) albums: Vec<YtAlbum>,
    liked: Option<Vec<TrackId>>,
    playlists: Vec<(PlaylistId, String, Option<String>, Vec<TrackId>)>,
    pub(crate) sources: HashMap<TrackId, String>,
    /// Albums saved in the user's YouTube Music library.
    saved_albums: HashSet<AlbumId>,
    errors: Vec<(String, String)>,
}

/// Covers on disk after the fetch step, with palettes for the ones fetched just now.
struct Covers {
    available: HashSet<String>,
    palettes: HashMap<String, Vec<String>>,
}

impl Covers {
    fn key(&self, key: &str) -> Option<String> {
        self.available.contains(key).then(|| key.to_owned())
    }

    fn palette(&self, key: &str) -> Vec<String> {
        self.palettes.get(key).cloned().unwrap_or_default()
    }
}

pub async fn run<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    plan: &SyncPlan,
    stage: impl Fn(SyncStage),
) -> Result<SyncReport, YtError> {
    let collected = collect(yt, plan, &stage).await?;
    stage(SyncStage::Covers { done: 0, total: 0 });
    let covers = fetch_covers(http, covers, &collected, &stage).await;
    stage(SyncStage::Saving);
    save(db, collected, covers, true).map_err(|e| YtError::Parse(format!("библиотека: {e}")))
}

async fn collect<Y: YtMusic>(yt: &Y, plan: &SyncPlan, stage: &impl Fn(SyncStage)) -> Result<Collected, YtError> {
    let mut c = Collected::default();
    // Auth failures abort the whole sync; anything else only skips that source.
    fn soft<T>(r: Result<T, YtError>, what: &str, errors: &mut Vec<(String, String)>) -> Result<Option<T>, YtError> {
        match r {
            Ok(v) => Ok(Some(v)),
            Err(YtError::Auth) => Err(YtError::Auth),
            Err(e) => {
                errors.push((what.to_owned(), e.to_string()));
                Ok(None)
            }
        }
    }
    fn add(c: &mut Collected, tracks: Vec<YtTrack>, source: &str) {
        for t in tracks {
            c.sources.entry(t.id.clone()).or_insert_with(|| source.to_owned());
            c.tracks.push(t);
        }
    }

    if plan.liked {
        stage(SyncStage::Liked);
        if let Some(liked) = soft(yt.liked_tracks().await, "liked", &mut c.errors)? {
            c.liked = Some(liked.iter().map(|t| t.id.clone()).collect());
            add(&mut c, liked, "liked");
        }
    }
    for id in &plan.playlist_ids {
        stage(SyncStage::Playlist(id.to_string()));
        if let Some(p) = soft(yt.playlist(id).await, &format!("playlist:{id}"), &mut c.errors)? {
            let ids = p.tracks.iter().map(|t| t.id.clone()).collect();
            c.playlists.push((p.card.id.clone(), p.card.title.clone(), p.card.thumbnail.clone(), ids));
            add(&mut c, p.tracks, &format!("playlist:{id}"));
        }
    }
    let mut album_ids: Vec<(AlbumId, &'static str)> = Vec::new();
    if plan.albums
        && let Some(saved) = soft(yt.library_albums().await, "albums", &mut c.errors)?
    {
        c.saved_albums.extend(saved.iter().map(|a| a.id.clone()));
        album_ids.extend(saved.into_iter().map(|a| (a.id, "album")));
    }
    if plan.artists
        && let Some(artists) = soft(yt.library_artists().await, "artists", &mut c.errors)?
    {
        let total = artists.len();
        for (i, a) in artists.iter().enumerate() {
            stage(SyncStage::Artists { done: i, total });
            if let Some(full) = soft(yt.artist(&a.id).await, &format!("artist:{}", a.id), &mut c.errors)? {
                album_ids.extend(full.discography.into_iter().map(|d| (d.id, "artist")));
            }
        }
    }
    let mut seen = HashSet::new();
    album_ids.retain(|(id, _)| seen.insert(id.clone()));
    let total = album_ids.len();
    for (i, (id, source)) in album_ids.into_iter().enumerate() {
        stage(SyncStage::Albums { done: i, total });
        if let Some(album) = soft(yt.album(&id).await, &format!("album:{id}"), &mut c.errors)? {
            add(&mut c, album.tracks.clone(), source);
            c.albums.push(album);
        }
    }
    Ok(c)
}

/// Cover key and source URL for each album and each album-less track.
fn cover_jobs(c: &Collected) -> Vec<(String, String)> {
    let mut jobs: HashMap<String, String> = HashMap::new();
    for a in &c.albums {
        if let Some(url) = &a.card.thumbnail {
            jobs.insert(a.card.id.to_string(), url.clone());
        }
    }
    for t in &c.tracks {
        let key = cover_key(t);
        if let Some(url) = &t.thumbnail {
            jobs.entry(key).or_insert_with(|| url.clone());
        }
    }
    for (id, _, thumb, _) in &c.playlists {
        if let Some(url) = thumb {
            jobs.insert(id.to_string(), url.clone());
        }
    }
    jobs.into_iter().collect()
}

/// An art track's thumbnail is its album cover, so album tracks share the album's key.
fn cover_key(t: &YtTrack) -> String {
    t.album.as_ref().map(|a| a.id.to_string()).unwrap_or_else(|| t.id.to_string())
}

/// Downloads missing covers. Covers already on disk are skipped and keep the palette
/// stored with their album.
async fn fetch_covers(http: &reqwest::Client, covers: &CoverStore, c: &Collected, stage: &impl Fn(SyncStage)) -> Covers {
    let (have, todo): (Vec<_>, Vec<_>) = cover_jobs(c).into_iter().partition(|(k, _)| covers.exists(k));
    let mut available: HashSet<String> = have.into_iter().map(|(k, _)| k).collect();
    let total = todo.len();
    let sem = Arc::new(tokio::sync::Semaphore::new(COVER_PARALLEL));
    let mut set = tokio::task::JoinSet::new();
    for (key, url) in todo {
        let (http, covers, sem) = (http.clone(), covers.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            let bytes = thumbs::download(&http, &url).await.ok()?;
            let processed =
                tokio::task::spawn_blocking(move || covers.process(&key, &bytes).map(|p| (key, p.palette))).await;
            processed.ok()?.ok()
        });
    }
    let mut palettes = HashMap::new();
    let mut done = 0;
    while let Some(res) = set.join_next().await {
        done += 1;
        stage(SyncStage::Covers { done, total });
        if let Ok(Some((key, palette))) = res {
            available.insert(key.clone());
            palettes.insert(key, palette);
        }
    }
    Covers { available, palettes }
}

fn save(db: &Db, c: Collected, covers: Covers, enqueue: bool) -> mixtape_library::Result<SyncReport> {
    let mut report = SyncReport { errors: c.errors.clone(), ..Default::default() };
    report.tracks = c.tracks.iter().map(|t| &t.id).collect::<HashSet<_>>().len();
    let queued = db.write(move |t| {
        // Full album pages first: they carry type, year and artists.
        for a in &c.albums {
            let key = a.card.id.to_string();
            repo::upsert_album(
                t,
                &AlbumMeta {
                    id: a.card.id.clone(),
                    title: a.card.title.clone(),
                    artists: a.card.artists.clone(),
                    year: a.card.year,
                    album_type: a.card.album_type.unwrap_or(AlbumType::Album),
                    cover_key: covers.key(&key),
                    palette: covers.palette(&key),
                    track_count: Some(a.tracks.len() as u32),
                },
            )?;
        }
        let mut queued = 0;
        let mut seen = HashSet::new();
        for tr in &c.tracks {
            if !seen.insert(tr.id.clone()) {
                continue;
            }
            let key = cover_key(tr);
            let cover = covers.key(&key);
            // Albums known only from a track row: create them once with what we have.
            if let Some(AlbumRef { id, title }) = &tr.album
                && !ytm::album_exists(t, id)?
            {
                repo::upsert_album(
                    t,
                    &AlbumMeta {
                        id: id.clone(),
                        title: title.clone(),
                        artists: tr.artists.clone(),
                        year: None,
                        album_type: if title == &tr.title { AlbumType::Single } else { AlbumType::Album },
                        cover_key: cover.clone(),
                        palette: covers.palette(&key),
                        track_count: None,
                    },
                )?;
            }
            repo::upsert_track(
                t,
                &TrackMeta {
                    id: tr.id.clone(),
                    title: tr.title.clone(),
                    artists: tr.artists.clone(),
                    album: tr.album.clone(),
                    track_no: tr.track_no,
                    duration_ms: tr.duration_ms,
                    cover_key: cover,
                },
            )?;
            let source = c.sources.get(&tr.id).map(String::as_str).unwrap_or("sync");
            if enqueue && jobs::enqueue(t, &tr.id, source)?.is_some() {
                queued += 1;
            }
        }
        for id in &c.saved_albums {
            ytm::set_album_in_library(t, id)?;
        }
        if let Some(liked) = &c.liked {
            ytm::apply_remote_likes(t, liked)?;
            ytm::source_synced(t, "liked", None)?;
        }
        for (id, title, thumb, items) in &c.playlists {
            let cover = thumb.as_ref().and_then(|_| covers.key(id.as_str()));
            ytm::mirror_playlist(t, id, title, cover.as_deref(), items)?;
        }
        for (what, err) in &c.errors {
            ytm::source_synced(t, what, Some(err))?;
        }
        ytm::link_artists_by_name(t)?;
        Ok(queued)
    })?;
    report.queued = queued;
    Ok(report)
}

/// Marks subscribed artists as library artists (shown on the Artists screen).
pub fn save_library_artists(db: &Db, artists: Vec<Artist>) -> mixtape_library::Result<()> {
    db.write(move |t| {
        for a in &artists {
            repo::upsert_artist(t, a)?;
        }
        Ok(())
    })
}

/// Fetches covers for what was collected and writes it; `enqueue` queues downloads.
pub(crate) async fn store(
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    c: Collected,
    enqueue: bool,
) -> mixtape_library::Result<SyncReport> {
    let covers = fetch_covers(http, covers, &c, &|_| {}).await;
    save(db, c, covers, enqueue)
}

/// Stores catalog tracks (a mix page, search results) with their covers, without queueing
/// downloads; they stay `remote` until played or saved. Returns their ids in order.
pub async fn store_tracks(
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    tracks: Vec<YtTrack>,
) -> mixtape_library::Result<Vec<TrackId>> {
    let ids = tracks.iter().map(|t| t.id.clone()).collect();
    let c = Collected { tracks, ..Default::default() };
    let covers = fetch_covers(http, covers, &c, &|_| {}).await;
    save(db, c, covers, false)?;
    Ok(ids)
}

/// Counts for the sync screen before anything is written.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPreview {
    pub liked: usize,
    pub playlists: Vec<mixtape_yt::api::YtPlaylistCard>,
    pub albums: usize,
    pub artists: usize,
}

pub async fn preview<Y: YtMusic>(yt: &Y) -> Result<SyncPreview, YtError> {
    let liked = yt.liked_tracks().await?.len();
    let playlists = yt.library_playlists().await?;
    let albums = yt.library_albums().await?.len();
    let artists = yt.library_artists().await?.len();
    Ok(SyncPreview { liked, playlists, albums, artists })
}

/// What a pasted link added, for the toast.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportResult {
    /// `track`, `album` or `playlist`.
    pub kind: String,
    pub title: String,
    pub track_count: u32,
}

/// Imports what a music.youtube.com / youtube.com link points at and queues its tracks.
pub async fn import_link<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    link: crate::url_import::Link,
) -> Result<ImportResult, YtError> {
    use crate::url_import::Link;
    let mut c = Collected::default();
    let result = match link {
        Link::Track(id) => {
            let t = yt.track(&id).await?;
            let title = t.title.clone();
            c.sources.insert(t.id.clone(), "url".into());
            c.tracks.push(t);
            ImportResult { kind: "track".into(), title, track_count: 1 }
        }
        Link::Album(id) => {
            let a = yt.album(&id).await?;
            for t in &a.tracks {
                c.sources.insert(t.id.clone(), "url".into());
            }
            c.tracks.extend(a.tracks.iter().cloned());
            let r = ImportResult { kind: "album".into(), title: a.card.title.clone(), track_count: a.tracks.len() as u32 };
            c.albums.push(a);
            r
        }
        Link::Playlist(id) => {
            let album_list = id.as_str().starts_with("OLAK");
            let p = yt.playlist(&id).await?;
            for t in &p.tracks {
                c.sources.insert(t.id.clone(), "url".into());
            }
            let ids = p.tracks.iter().map(|t| t.id.clone()).collect();
            // An `OLAK5uy_…` list is an album's audio playlist, not something to mirror.
            if !album_list {
                c.playlists.push((p.card.id.clone(), p.card.title.clone(), p.card.thumbnail.clone(), ids));
            }
            let r = ImportResult {
                kind: if album_list { "album" } else { "playlist" }.into(),
                title: p.card.title.clone(),
                track_count: p.tracks.len() as u32,
            };
            c.tracks.extend(p.tracks);
            r
        }
        Link::Artist(_) => {
            return Err(YtError::Unavailable(
                "ссылки на исполнителей пока не поддерживаются — добавьте альбом или плейлист".into(),
            ));
        }
    };
    let covers = fetch_covers(http, covers, &c, &|_| {}).await;
    save(db, c, covers, true).map_err(|e| YtError::Parse(format!("библиотека: {e}")))?;
    Ok(result)
}
