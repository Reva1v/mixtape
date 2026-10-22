//! Source sync (T060): reads the chosen YouTube Music sources, writes their metadata to
//! the library, fetches covers and queues downloads for tracks not yet in the library.

use mixtape_artwork::{CoverStore, Detail};
use mixtape_core::{AlbumId, AlbumRef, AlbumType, Artist, ArtistId, PlaylistId, TrackId};
use mixtape_library::repo::{self, AlbumMeta, TrackMeta};
use mixtape_library::{Db, jobs, ytm};
use mixtape_yt::api::{YtAlbum, YtMusic, YtTrack};
use mixtape_yt::{YtError, thumbs};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// What the user chose on the sync screen; kept for the scheduled sync.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
/// A subscribed artist's discography is read again after this long.
const ARTIST_RESYNC_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// `sync_sources.kind` of a whole scheduled or manual sync that reached YouTube.
pub const PLAN_SOURCE: &str = "plan";

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
    /// Artists whose page (discography) was read just now.
    artists_read: Vec<(ArtistId, String)>,
    errors: Vec<(String, String)>,
    /// A source failed for lack of network: the sync is not counted as done.
    network_failed: bool,
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
    sync(yt, http, db, covers, plan, stage, true).await
}

/// [`run`] without queueing downloads: the library's metadata and covers only, for a
/// device that streams what it plays (the Android app).
pub async fn run_metadata<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    plan: &SyncPlan,
    stage: impl Fn(SyncStage),
) -> Result<SyncReport, YtError> {
    sync(yt, http, db, covers, plan, stage, false).await
}

async fn sync<Y: YtMusic>(
    yt: &Y,
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    plan: &SyncPlan,
    stage: impl Fn(SyncStage),
    enqueue: bool,
) -> Result<SyncReport, YtError> {
    let collected = collect(yt, db, plan, &stage).await?;
    let reached = !collected.network_failed;
    stage(SyncStage::Covers { done: 0, total: 0 });
    let covers = fetch_covers(http, covers, &collected, Detail::Full, &stage).await;
    stage(SyncStage::Saving);
    let report = save(db, collected, covers, enqueue).map_err(crate::catalog::lib_err)?;
    // The schedule counts from here; a sync cut short by the network is tried again sooner.
    if reached {
        let _ = db.write(|t| ytm::source_synced(t, PLAN_SOURCE, None));
    }
    Ok(report)
}

/// Reads the plan's sources. Albums whose tracks are all held already are not read again,
/// and a subscribed artist's discography is read at most once a week; liked tracks and
/// playlists are read every time.
async fn collect<Y: YtMusic>(yt: &Y, db: &Db, plan: &SyncPlan, stage: &impl Fn(SyncStage)) -> Result<Collected, YtError> {
    let mut c = Collected::default();
    // Auth failures abort the whole sync; anything else only skips that source.
    fn soft<T>(r: Result<T, YtError>, what: &str, c: &mut Collected) -> Result<Option<T>, YtError> {
        match r {
            Ok(v) => Ok(Some(v)),
            Err(YtError::Auth) => Err(YtError::Auth),
            Err(e) => {
                c.network_failed |= matches!(e, YtError::Network(_) | YtError::RateLimited);
                c.errors.push((what.to_owned(), e.to_string()));
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
        if let Some(liked) = soft(yt.liked_tracks().await, "liked", &mut c)? {
            c.liked = Some(liked.iter().map(|t| t.id.clone()).collect());
            add(&mut c, liked, "liked");
        }
    }
    for id in &plan.playlist_ids {
        if c.network_failed {
            break;
        }
        stage(SyncStage::Playlist(id.to_string()));
        if let Some(p) = soft(yt.playlist(id).await, &format!("playlist:{id}"), &mut c)? {
            let ids = p.tracks.iter().map(|t| t.id.clone()).collect();
            c.playlists.push((p.card.id.clone(), p.card.title.clone(), p.card.thumbnail.clone(), ids));
            add(&mut c, p.tracks, &format!("playlist:{id}"));
        }
    }
    let mut album_ids: Vec<(AlbumId, &'static str)> = Vec::new();
    if plan.albums
        && !c.network_failed
        && let Some(saved) = soft(yt.library_albums().await, "albums", &mut c)?
    {
        c.saved_albums.extend(saved.iter().map(|a| a.id.clone()));
        album_ids.extend(saved.into_iter().map(|a| (a.id, "album")));
    }
    if plan.artists
        && !c.network_failed
        && let Some(artists) = soft(yt.library_artists().await, "artists", &mut c)?
    {
        let fresh_since = repo::now_ms() - ARTIST_RESYNC_MS;
        let due: Vec<_> = artists
            .into_iter()
            .filter(|a| db.read(|conn| ytm::artist_synced_at(conn, &a.id)).ok().flatten().is_none_or(|t| t < fresh_since))
            .collect();
        let total = due.len();
        for (i, a) in due.into_iter().enumerate() {
            if c.network_failed {
                break;
            }
            stage(SyncStage::Artists { done: i, total });
            if let Some(full) = soft(yt.artist(&a.id).await, &format!("artist:{}", a.id), &mut c)? {
                album_ids.extend(full.discography.into_iter().map(|d| (d.id, "artist")));
                c.artists_read.push((a.id, a.name));
            }
        }
    }
    let mut seen = HashSet::new();
    album_ids.retain(|(id, _)| seen.insert(id.clone()) && !db.read(|conn| ytm::album_synced(conn, id)).unwrap_or(false));
    let total = album_ids.len();
    for (i, (id, source)) in album_ids.into_iter().enumerate() {
        if c.network_failed {
            break;
        }
        stage(SyncStage::Albums { done: i, total });
        if let Some(album) = soft(yt.album(&id).await, &format!("album:{id}"), &mut c)? {
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

/// Downloads covers missing at `detail` (a small cover of something now in the library
/// is fetched again in full). Covers already on disk are skipped and keep the palette
/// stored with their album.
async fn fetch_covers(
    http: &reqwest::Client,
    covers: &CoverStore,
    c: &Collected,
    detail: Detail,
    stage: &impl Fn(SyncStage),
) -> Covers {
    let (have, todo): (Vec<_>, Vec<_>) = cover_jobs(c).into_iter().partition(|(k, _)| covers.has(k, detail));
    let mut available: HashSet<String> = have.into_iter().map(|(k, _)| k).collect();
    // A small cover stays usable if its upgrade fails.
    available.extend(todo.iter().filter(|(k, _)| covers.exists(k)).map(|(k, _)| k.clone()));
    let total = todo.len();
    let sem = Arc::new(tokio::sync::Semaphore::new(COVER_PARALLEL));
    let mut set = tokio::task::JoinSet::new();
    for (key, url) in todo {
        let (http, covers, sem) = (http.clone(), covers.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            let bytes = match detail {
                Detail::Full => thumbs::download(&http, &url).await,
                Detail::Small => thumbs::download_small(&http, &url).await,
            };
            let bytes = bytes.ok()?;
            let processed =
                tokio::task::spawn_blocking(move || covers.process(&key, &bytes, detail).map(|p| (key, p.palette)))
                    .await;
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
        for (id, name) in &c.artists_read {
            ytm::set_artist_synced(t, id, name)?;
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

/// Fetches covers for what was collected and writes it; `enqueue` queues downloads (and
/// so keeps full-size covers, the rest is only browsed and gets small ones).
pub(crate) async fn store(
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    c: Collected,
    enqueue: bool,
) -> mixtape_library::Result<SyncReport> {
    let detail = if enqueue { Detail::Full } else { Detail::Small };
    let covers = fetch_covers(http, covers, &c, detail, &|_| {}).await;
    save(db, c, covers, enqueue)
}

/// Stores catalog tracks (a mix page, search results) with small covers, without queueing
/// downloads; they stay `remote` until played or saved. Returns their ids in order.
pub async fn store_tracks(
    http: &reqwest::Client,
    db: &Db,
    covers: &CoverStore,
    tracks: Vec<YtTrack>,
) -> mixtape_library::Result<Vec<TrackId>> {
    let ids = tracks.iter().map(|t| t.id.clone()).collect();
    let c = Collected { tracks, ..Default::default() };
    let covers = fetch_covers(http, covers, &c, Detail::Small, &|_| {}).await;
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
        Link::Artist(id) => {
            // The whole discography, album by album (each one is stored and queued).
            let name = yt.artist(&id).await?.card.name;
            let n = crate::catalog::download_artist(yt, http, db, covers, &id).await?;
            return Ok(ImportResult { kind: "artist".into(), title: name, track_count: n as u32 });
        }
    };
    let covers = fetch_covers(http, covers, &c, Detail::Full, &|_| {}).await;
    save(db, c, covers, true).map_err(crate::catalog::lib_err)?;
    Ok(result)
}
