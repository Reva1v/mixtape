//! SoundCloud and Bandcamp imports through the bundled yt-dlp: no account, no ffmpeg.
//!
//! [`list`] runs `yt-dlp -J --flat-playlist` once for the link's title and track list; then
//! [`import`] downloads the tracks one at a time (one yt-dlp process each, nothing runs in
//! between imports) straight into `library/<id>.<ext>`, the way the YouTube downloader stores
//! files. Only formats the audio engine decodes without transcoding are picked ([`FORMATS`]):
//! Bandcamp's MP3 (or AAC; a free "name your price" download gives MP3 V0/320, otherwise the
//! 128k stream) and SoundCloud's HLS AAC (or MP3), which yt-dlp's native HLS downloader joins
//! into a fragmented MP4 (its per-segment `sidx` boxes are neutralised so the whole file
//! plays). SoundCloud's original-file downloads need a login, so they are not used; Go+ tracks
//! only have a 30-second preview without a subscription and are refused ([`is_preview`]).
//!
//! Track ids are `sc`/`bc` + a hash of the site's own track id, so the same track imported
//! from a single link, a set or an album is one row. Bandcamp albums become albums (the album
//! id hashes the artist subdomain and the album title, so a track link lands in the same
//! album); a SoundCloud set becomes a local playlist, and its tracks are credited to it as an
//! album when SoundCloud marks the set as an album or EP. Everything else is a single.

use crate::url_import::{ExternalKind, ExternalLink, Site, parse_external};
use md5::{Digest, Md5};
use mixtape_artwork::{CoverStore, Detail};
use mixtape_core::{
    AlbumId, AlbumRef, AlbumType, ArtistRef, PlaylistId, TrackId, TrackState, tr, trf,
};
use mixtape_library::repo::{self, AlbumMeta, FileLocation, MediaFile, TrackMeta};
use mixtape_library::{Db, playlists, settings, ytm};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// Bandcamp: free-download MP3 V0 / 320 / AAC, else the 128k stream. SoundCloud: HLS AAC
/// 160k, progressive or HLS MP3, then the smaller AAC / Opus streams. Never ALAC/FLAC/WAV
/// (big, and not all decodable) and never anything that would need ffmpeg.
pub const FORMATS: &str = "mp3-v0/mp3-320/aac-hi/mp3-128/hls_aac_160k/http_mp3_128/hls_mp3_128/\
hls_aac_96k/hls_opus_64/bestaudio[ext=mp3]/bestaudio[acodec^=mp4a]/bestaudio[ext=opus]";
/// Uploads taken from a SoundCloud profile link (newest first).
pub const PROFILE_LIMIT: u32 = 100;
const LIST_TIMEOUT: Duration = Duration::from_secs(120);
const TRACK_TIMEOUT: Duration = Duration::from_secs(600);

/// A SoundCloud set that SoundCloud marks as an album or EP.
#[derive(Debug, Clone, PartialEq)]
pub struct SetAlbum {
    pub id: AlbumId,
    pub title: String,
    pub artist: Option<String>,
    pub album_type: AlbumType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: TrackId,
    pub url: String,
}

/// What a link holds, from `yt-dlp -J --flat-playlist`.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub link: ExternalLink,
    pub title: String,
    pub entries: Vec<Entry>,
    pub set_album: Option<SetAlbum>,
}

impl Listing {
    /// `track`, `album`, `playlist` or `artist`, as `sources::ImportResult` names them.
    pub fn kind_name(&self) -> &'static str {
        match self.link.kind {
            ExternalKind::Track => "track",
            ExternalKind::Album => "album",
            ExternalKind::Set => "playlist",
            ExternalKind::Profile => "artist",
        }
    }
}

/// Where downloads and rows go.
pub struct Target<'a> {
    pub ytdlp: &'a Path,
    /// The data directory; files land in `library/`.
    pub root: &'a Path,
    pub http: &'a reqwest::Client,
    pub db: &'a Db,
    pub covers: &'a CoverStore,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    /// Newly downloaded tracks.
    pub added: u32,
    /// Tracks that were in the library already.
    pub present: u32,
    /// (track url, reason in the app language).
    pub failed: Vec<(String, String)>,
    /// The local playlist a SoundCloud set was saved as.
    pub playlist: Option<PlaylistId>,
}

pub fn site_name(site: Site) -> &'static str {
    match site {
        Site::SoundCloud => "SoundCloud",
        Site::Bandcamp => "Bandcamp",
    }
}

fn hash16(s: &str) -> String {
    let digest = Md5::digest(s.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// `sc…` / `bc…` from the site's own track id (18 characters, never mistaken for a video id).
pub fn track_id(site: Site, site_id: &str) -> TrackId {
    match site {
        Site::SoundCloud => TrackId(format!("sc{}", hash16(&format!("soundcloud:{site_id}")))),
        Site::Bandcamp => TrackId(format!("bc{}", hash16(&format!("bandcamp:{site_id}")))),
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Only https links on the site's own hosts go back to yt-dlp: the listing is remote data
/// and could otherwise point it at any URL (and any of its extractors). Returns the URL
/// normalised.
pub fn safe_url(site: Site, raw: &str) -> Option<String> {
    let u = url::Url::parse(raw).ok()?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
    {
        return None;
    }
    let host = u.host_str()?.to_ascii_lowercase();
    let on_site = |domain: &str| {
        host == domain
            || host
                .strip_suffix(domain)
                .is_some_and(|sub| sub.ends_with('.') && sub.len() > 1)
    };
    let ok = match site {
        Site::SoundCloud => on_site("soundcloud.com"),
        Site::Bandcamp => on_site("bandcamp.com"),
    };
    ok.then(|| u.into())
}

/// yt-dlp ids are strings, but be lenient.
fn id_of(v: &Value) -> Option<String> {
    match v.get("id")? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn nothing_found() -> String {
    tr(
        "There are no tracks at this link",
        "За цим посиланням немає треків",
        "По этой ссылке нет треков",
    )
    .into()
}

/// Reads `yt-dlp -J --flat-playlist` output for `link`.
pub fn parse_listing(link: &ExternalLink, v: &Value) -> Result<Listing, String> {
    let site = link.site;
    let is_list = str_of(v, "_type") == Some("playlist") || v.get("entries").is_some();
    if link.kind == ExternalKind::Track || !is_list {
        let id = id_of(v).ok_or_else(nothing_found)?;
        let title = str_of(v, "track")
            .or(str_of(v, "title"))
            .unwrap_or_default()
            .to_owned();
        let url = str_of(v, "webpage_url")
            .and_then(|u| safe_url(site, u))
            .unwrap_or_else(|| link.url.clone());
        return Ok(Listing {
            link: ExternalLink {
                kind: ExternalKind::Track,
                ..link.clone()
            },
            title,
            entries: vec![Entry {
                id: track_id(site, &id),
                url,
            }],
            set_album: None,
        });
    }
    let mut seen = HashSet::new();
    let entries: Vec<Entry> = v
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let id = id_of(e)?;
            let url = str_of(e, "url").or(str_of(e, "webpage_url"))?;
            let Some(url) = safe_url(site, url) else {
                tracing::warn!(url, "external import: entry link off the site, skipped");
                return None;
            };
            let id = track_id(site, &id);
            seen.insert(id.clone()).then_some(Entry { id, url })
        })
        .collect();
    if entries.is_empty() {
        return Err(nothing_found());
    }
    let mut title = str_of(v, "album")
        .or(str_of(v, "title"))
        .unwrap_or_default()
        .to_owned();
    if link.kind == ExternalKind::Profile
        && let Some((name, _)) = title.strip_suffix(')').and_then(|t| t.rsplit_once(" ("))
    {
        // "Forss (Tracks)".
        title = name.to_owned();
    }
    let set_album = (link.kind == ExternalKind::Set)
        .then(|| {
            let album_type = match str_of(v, "album_type")? {
                "album" | "compilation" => AlbumType::Album,
                "ep" => AlbumType::Ep,
                "single" => AlbumType::Single,
                _ => return None,
            };
            let set_id = id_of(v).unwrap_or_else(|| link.url.clone());
            Some(SetAlbum {
                id: AlbumId(format!(
                    "scalb{}",
                    hash16(&format!("soundcloud-set:{set_id}"))
                )),
                title: title.clone(),
                artist: str_of(v, "album_artist")
                    .or(str_of(v, "uploader"))
                    .map(str::to_owned),
                album_type,
            })
        })
        .flatten();
    Ok(Listing {
        link: link.clone(),
        title,
        entries,
        set_album,
    })
}

/// A track's rows, from the info JSON yt-dlp prints after downloading it.
#[derive(Debug, Clone)]
pub struct Mapped {
    pub track: TrackMeta,
    pub album: AlbumMeta,
    pub cover_url: Option<String>,
}

/// "Artist - Title" in a SoundCloud title (uploads by labels and reposting channels).
fn split_artist(title: &str) -> Option<(&str, &str)> {
    [" - ", " – ", " — "].iter().find_map(|sep| {
        let (a, t) = title.split_once(sep)?;
        let (a, t) = (a.trim(), t.trim());
        (!a.is_empty() && !t.is_empty()).then_some((a, t))
    })
}

fn year_of(v: &Value) -> Option<u16> {
    v.get("release_year")
        .and_then(Value::as_u64)
        .or_else(|| str_of(v, "upload_date").and_then(|d| d.get(..4)?.parse().ok()))
        .and_then(|y| u16::try_from(y).ok())
}

fn credit(name: &str) -> ArtistRef {
    ArtistRef {
        id: None,
        name: name.to_owned(),
    }
}

/// Maps a downloaded track's info. `set` and `position` (1-based) come from the set it was
/// imported with, if any.
pub fn map_track(
    site: Site,
    id: &TrackId,
    v: &Value,
    set: Option<&SetAlbum>,
    position: u32,
) -> Mapped {
    let unknown = tr(
        "Unknown artist",
        "Невідомий виконавець",
        "Неизвестный исполнитель",
    );
    let raw_title = str_of(v, "title").unwrap_or(tr("Untitled", "Без назви", "Без названия"));
    let uploader = str_of(v, "uploader");
    let (title, artist, album) = match site {
        Site::SoundCloud => {
            let (artist, title) = match split_artist(raw_title) {
                Some((a, t)) => (a, t),
                None => (
                    str_of(v, "artist").or(uploader).unwrap_or(unknown),
                    raw_title,
                ),
            };
            let album = set.map(|s| {
                let artist = s.artist.as_deref().unwrap_or(artist);
                (
                    s.id.clone(),
                    s.title.clone(),
                    artist.to_owned(),
                    s.album_type,
                    Some(position),
                )
            });
            (title, artist, album)
        }
        Site::Bandcamp => {
            let title = str_of(v, "track").unwrap_or(raw_title);
            let artist = str_of(v, "artist").or(uploader).unwrap_or(unknown);
            let album = str_of(v, "album").map(|name| {
                let owner = str_of(v, "uploader_id")
                    .or(uploader)
                    .unwrap_or(artist)
                    .to_lowercase();
                let id = AlbumId(format!(
                    "bcalb{}",
                    hash16(&format!("bandcamp-album:{owner}:{}", name.to_lowercase()))
                ));
                let album_artist = str_of(v, "album_artist").unwrap_or(artist).to_owned();
                let no = v
                    .get("track_number")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32);
                (id, name.to_owned(), album_artist, AlbumType::Album, no)
            });
            (title, artist, album)
        }
    };
    let (album_id, album_title, album_artist, album_type, track_no) = album.unwrap_or_else(|| {
        let prefix = if site == Site::SoundCloud {
            "scalb"
        } else {
            "bcalb"
        };
        (
            AlbumId(format!("{prefix}{}", &id.as_str()[2..])),
            title.to_owned(),
            artist.to_owned(),
            AlbumType::Single,
            Some(1),
        )
    });
    let cover_url = str_of(v, "thumbnail").map(|u| match site {
        // `-original` can be several megabytes; 500 px is plenty for 300/1024 covers.
        Site::SoundCloud => u.replace("-original.", "-t500x500."),
        Site::Bandcamp => u.to_owned(),
    });
    let cover_key = Some(album_id.0.clone());
    let duration_ms = v
        .get("duration")
        .and_then(Value::as_f64)
        .map(|d| (d * 1000.0).round() as u64)
        .unwrap_or(0);
    let track = TrackMeta {
        id: id.clone(),
        title: title.to_owned(),
        artists: vec![credit(artist)],
        album: Some(AlbumRef {
            id: album_id.clone(),
            title: album_title.clone(),
        }),
        track_no,
        duration_ms,
        cover_key: cover_key.clone(),
    };
    let album = AlbumMeta {
        id: album_id,
        title: album_title,
        artists: vec![credit(&album_artist)],
        year: year_of(v),
        album_type,
        cover_key,
        palette: Vec::new(),
        track_count: (album_type == AlbumType::Single).then_some(1),
    };
    Mapped {
        track,
        album,
        cover_url,
    }
}

/// The reason from yt-dlp's last `ERROR:` line, without the `[extractor] id:` prefix.
fn ytdlp_error(stderr: &str) -> String {
    let line = stderr
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("ERROR:"))
        .map(str::trim);
    let reason = line
        .map(
            |l| match l.strip_prefix('[').and_then(|r| r.split_once("] ")) {
                Some((_, rest)) => match rest.split_once(": ") {
                    Some((id, msg)) if !id.contains(' ') => msg,
                    _ => rest,
                },
                None => l,
            },
        )
        .unwrap_or("no answer");
    trf!("yt-dlp: {reason}", "yt-dlp: {reason}", "yt-dlp: {reason}")
}

/// yt-dlp without config files or plugins, running in `work_dir` (see
/// [`mixtape_yt::resolver::ytdlp_command`]).
fn ytdlp_cmd(ytdlp: &Path, work_dir: &Path) -> Command {
    let mut cmd = mixtape_yt::resolver::ytdlp_command(ytdlp, work_dir);
    cmd.args(["--no-warnings", "--no-progress", "--encoding", "utf-8"]);
    cmd
}

/// The link's title and tracks (one yt-dlp process). It writes nothing; it runs in the
/// system temporary folder.
pub async fn list(ytdlp: &Path, link: &ExternalLink) -> Result<Listing, String> {
    let link = &parse_external(&link.url)
        .filter(|l| l.site == link.site)
        .ok_or_else(|| tr("Unsupported link", "Непідтримуване посилання", "Неподдерживаемая ссылка").to_owned())?;
    let mut cmd = ytdlp_cmd(ytdlp, &std::env::temp_dir());
    cmd.args(["-J", "--flat-playlist"]);
    if link.kind == ExternalKind::Profile {
        cmd.arg("--playlist-end").arg(PROFILE_LIMIT.to_string());
    }
    cmd.arg("--").arg(&link.url);
    let (out, err) = mixtape_yt::resolver::run_capture(cmd, LIST_TIMEOUT).await?;
    let v: Value = serde_json::from_str(out.trim()).map_err(|_| ytdlp_error(&err))?;
    parse_listing(link, &v)
}

/// Downloads one track to `library/<id>.<ext>`; returns yt-dlp's info and the file.
async fn download(
    ytdlp: &Path,
    root: &Path,
    id: &TrackId,
    url: &str,
) -> Result<(Value, PathBuf), String> {
    if !id.is_safe() {
        return Err(format!("invalid track id {:?}", id.as_str()));
    }
    let dir = root.join("library");
    let work = root.join("tmp");
    for d in [&dir, &work] {
        tokio::fs::create_dir_all(d)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut cmd = ytdlp_cmd(ytdlp, &work);
    cmd.args([
        "-f",
        FORMATS,
        "--no-playlist",
        "--fixup",
        "never",
        "--downloader",
        "m3u8:native",
    ])
    .args(["--no-mtime", "--print", "after_move:%()j", "-o"])
    .arg(dir.join(format!("{id}.%(ext)s")))
    .arg("--")
    .arg(url);
    let (out, err) = mixtape_yt::resolver::run_capture(cmd, TRACK_TIMEOUT).await?;
    let info: Value = out
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l.trim()).ok())
        .ok_or_else(|| ytdlp_error(&err))?;
    let file = str_of(&info, "filepath")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .ok_or_else(|| ytdlp_error(&err))?;
    // The path comes from yt-dlp's output; it is rewritten and deleted later, so it must be
    // the file this call asked for.
    let file = library_file(&dir, id, &file).ok_or_else(|| {
        format!("yt-dlp wrote an unexpected file: {}", file.display())
    })?;
    Ok((info, file))
}

/// `file` resolved, when it is `<dir>/<id>.<ext>` (no links or `..` leading elsewhere).
fn library_file(dir: &Path, id: &TrackId, file: &Path) -> Option<PathBuf> {
    let dir = std::fs::canonicalize(dir).ok()?;
    let file = std::fs::canonicalize(file).ok()?;
    let name = file.file_name()?.to_str()?;
    let ext = name.strip_prefix(id.as_str())?.strip_prefix('.')?;
    let plain_ext = !ext.is_empty() && ext.len() <= 8 && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    (file.parent()? == dir && plain_ext && file.is_file()).then_some(file)
}

/// Downloads the listing's tracks one by one and stores them; `progress(done, total)` after
/// each. A track that fails is reported and skipped. Already downloaded tracks are kept.
pub async fn import(
    t: &Target<'_>,
    listing: &Listing,
    mut progress: impl FnMut(u32, u32),
) -> ImportReport {
    let total = listing.entries.len() as u32;
    let mut report = ImportReport::default();
    let mut order = Vec::new();
    for (i, entry) in listing.entries.iter().enumerate() {
        progress(i as u32, total);
        let state =
            t.db.read(|c| repo::track(c, &entry.id))
                .ok()
                .flatten()
                .map(|x| x.state);
        if state == Some(TrackState::Downloaded) {
            report.present += 1;
            order.push(entry.id.clone());
            continue;
        }
        match import_one(t, listing, entry, i as u32 + 1).await {
            Ok(()) => {
                report.added += 1;
                order.push(entry.id.clone());
            }
            Err(e) => {
                tracing::warn!(url = %entry.url, error = %e, "external import: track failed");
                report.failed.push((entry.url.clone(), e));
            }
        }
    }
    progress(total, total);
    // Imported credits have names only; give them artist ids.
    let _ = t.db.write(|c| ytm::link_artists_by_name(c).map(drop));
    if listing.link.kind == ExternalKind::Set && !order.is_empty() {
        match save_playlist(t.db, listing, order) {
            Ok(id) => report.playlist = Some(id),
            Err(e) => report
                .failed
                .push((listing.link.url.clone(), e.to_string())),
        }
    }
    report
}

/// SoundCloud Go+ tracks give anonymous listeners a 30-second `…_preview` format only.
fn is_preview(info: &Value, file_ms: u64) -> bool {
    let full_ms = info.get("duration").and_then(Value::as_f64).unwrap_or(0.0) * 1000.0;
    str_of(info, "format_id").is_some_and(|f| f.contains("preview"))
        || (full_ms > 60_000.0 && (file_ms as f64) * 2.0 < full_ms)
}

async fn import_one(
    t: &Target<'_>,
    listing: &Listing,
    entry: &Entry,
    position: u32,
) -> Result<(), String> {
    let site = listing.link.site;
    let (info, file) = download(t.ytdlp, t.root, &entry.id, &entry.url).await?;
    let probe_path = file.clone();
    let probed = tokio::task::spawn_blocking(move || {
        // Joined HLS segments carry an index per segment; see `drop_segment_indexes`.
        if let Err(e) = mixtape_audio::drop_segment_indexes(&probe_path) {
            tracing::warn!(error = %e, "external import: segment indexes");
        }
        mixtape_audio::probe(&probe_path)
    })
    .await
    .map_err(|e| e.to_string())?;
    let probe = match probed {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_file(&file);
            return Err(trf!(
                "Can't play the downloaded file: {e}",
                "Не вдається відтворити завантажений файл: {e}",
                "Не получается воспроизвести скачанный файл: {e}"
            ));
        }
    };
    let mut m = map_track(site, &entry.id, &info, listing.set_album.as_ref(), position);
    if is_preview(&info, probe.duration_ms) {
        let _ = std::fs::remove_file(&file);
        return Err(tr(
            "Only a preview is available without a subscription",
            "Без підписки доступний лише уривок",
            "Без подписки доступен только отрывок",
        )
        .into());
    }
    m.track.duration_ms = probe.duration_ms;
    if let Some(set) = &listing.set_album {
        if m.album.id == set.id {
            m.album.track_count = Some(listing.entries.len() as u32);
        }
    } else if listing.link.kind == ExternalKind::Album {
        m.album.track_count = Some(listing.entries.len() as u32);
    }
    let key = m.album.id.0.clone();
    if !t.covers.has(&key, Detail::Full)
        && let Some(url) = &m.cover_url
    {
        match mixtape_yt::thumbs::download(t.http, url).await {
            Ok(bytes) => match t.covers.process(&key, &bytes, Detail::Full) {
                Ok(p) => m.album.palette = p.palette,
                Err(e) => tracing::warn!(error = %e, "external import: cover"),
            },
            Err(e) => tracing::warn!(error = %e, "external import: cover download"),
        }
    }
    if !t.covers.exists(&key) {
        m.album.cover_key = None;
        m.track.cover_key = None;
    }
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let media = MediaFile {
        track_id: entry.id.clone(),
        path: format!("library/{name}"),
        container: probe.container.clone(),
        itag: None,
        size_bytes: std::fs::metadata(&file).map(|x| x.len()).unwrap_or(0),
        sample_rate: Some(probe.sample_rate),
        location: FileLocation::Library,
    };
    let Mapped { track, album, .. } = m;
    t.db.write(move |c| {
        repo::upsert_album(c, &album)?;
        repo::upsert_track(c, &track)?;
        repo::put_media_file(c, &media)
    })
    .map_err(|e| e.to_string())
}

/// A set is saved as a local playlist; importing it again refreshes the same playlist.
fn save_playlist(
    db: &Db,
    listing: &Listing,
    order: Vec<TrackId>,
) -> mixtape_library::Result<PlaylistId> {
    let key = format!("ext_playlist:{}", listing.link.url);
    let known: Option<PlaylistId> = settings::get(db, &key)?;
    let title = listing.title.clone();
    let id = db.write(move |c| {
        if let Some(id) = known
            && repo::playlist(c, &id)?.is_some()
        {
            playlists::rewrite(c, &id, &order)?;
            return Ok(id);
        }
        let id = playlists::create(c, &title)?;
        playlists::add(c, &id, &order)?;
        Ok(id)
    })?;
    settings::set(db, &key, &id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn ids_are_stable_and_not_video_ids() {
        let a = track_id(Site::SoundCloud, "293");
        assert_eq!(a, track_id(Site::SoundCloud, "293"));
        assert_ne!(a, track_id(Site::Bandcamp, "293"));
        assert!(
            a.as_str().starts_with("sc") && a.as_str().len() == 18,
            "{a:?}"
        );
        assert!(track_id(Site::Bandcamp, "1").as_str().starts_with("bc"));
    }

    #[test]
    fn soundcloud_track() {
        let link = parse_external("https://soundcloud.com/forss/flickermood").unwrap();
        let v = fixture("ytdlp_sc_track.json");
        let l = parse_listing(&link, &v).unwrap();
        assert_eq!(
            (l.title.as_str(), l.kind_name(), l.entries.len()),
            ("Flickermood", "track", 1)
        );
        assert_eq!(l.entries[0].id, track_id(Site::SoundCloud, "293"));
        assert_eq!(l.entries[0].url, "https://soundcloud.com/forss/flickermood");

        let m = map_track(Site::SoundCloud, &l.entries[0].id, &v, None, 1);
        assert_eq!(m.track.title, "Flickermood");
        assert_eq!(m.track.artists[0].name, "Forss");
        assert_eq!(m.track.duration_ms, 213_886);
        assert_eq!(m.album.album_type, AlbumType::Single);
        assert_eq!(m.album.title, "Flickermood");
        assert_eq!(m.album.year, Some(2003));
        assert_eq!(m.track.cover_key.as_deref(), Some(m.album.id.as_str()));
        assert!(m.album.id.as_str().starts_with("scalb"));
        assert_eq!(
            m.cover_url.as_deref(),
            Some("https://i1.sndcdn.com/artworks-000067273316-smsiqx-t500x500.jpg")
        );
    }

    #[test]
    fn soundcloud_title_with_artist() {
        let mut v = fixture("ytdlp_sc_track.json");
        v["title"] = "Daft Punk - Around the World (Edit)".into();
        let id = track_id(Site::SoundCloud, "293");
        let m = map_track(Site::SoundCloud, &id, &v, None, 1);
        assert_eq!(
            (m.track.title.as_str(), m.track.artists[0].name.as_str()),
            ("Around the World (Edit)", "Daft Punk")
        );
        v["title"] = "Intro -".into();
        let m = map_track(Site::SoundCloud, &id, &v, None, 1);
        assert_eq!(
            (m.track.title.as_str(), m.track.artists[0].name.as_str()),
            ("Intro -", "Forss")
        );
    }

    #[test]
    fn soundcloud_set_is_an_ep() {
        let link =
            parse_external("https://soundcloud.com/the-concept-band/sets/the-royal-concept-ep")
                .unwrap();
        let l = parse_listing(&link, &fixture("ytdlp_sc_set_flat.json")).unwrap();
        assert_eq!(
            (l.title.as_str(), l.kind_name()),
            ("The Royal Concept EP", "playlist")
        );
        assert_eq!(l.entries.len(), 3);
        assert_eq!(l.entries[0].id, track_id(Site::SoundCloud, "75206121"));
        assert_eq!(
            l.entries[0].url,
            "https://soundcloud.com/the-concept-band/world-on-fire-1"
        );
        let set = l.set_album.clone().unwrap();
        assert_eq!(
            (set.title.as_str(), set.artist.as_deref(), set.album_type),
            (
                "The Royal Concept EP",
                Some("The Royal Concept"),
                AlbumType::Ep
            )
        );

        // A track of the set is credited to the set as an album, numbered by position.
        let v = fixture("ytdlp_sc_track.json");
        let m = map_track(Site::SoundCloud, &l.entries[1].id, &v, Some(&set), 2);
        assert_eq!(
            m.track.album.as_ref().map(|a| a.id.clone()),
            Some(set.id.clone())
        );
        assert_eq!(
            (m.track.track_no, m.album.album_type),
            (Some(2), AlbumType::Ep)
        );
        assert_eq!(m.album.artists[0].name, "The Royal Concept");
        assert_eq!(m.track.artists[0].name, "Forss");
    }

    #[test]
    fn soundcloud_playlist_set_has_no_album() {
        let link = parse_external("https://soundcloud.com/u/sets/mix").unwrap();
        let mut v = fixture("ytdlp_sc_set_flat.json");
        v.as_object_mut().unwrap().remove("album_type");
        v.as_object_mut().unwrap().remove("album");
        let l = parse_listing(&link, &v).unwrap();
        assert_eq!(l.set_album, None);
        assert_eq!(l.title, "The Royal Concept EP");
    }

    #[test]
    fn bandcamp_track_and_album_share_the_album() {
        let link = parse_external("https://benprunty.bandcamp.com/track/lanius-battle").unwrap();
        let v = fixture("ytdlp_bc_track.json");
        let l = parse_listing(&link, &v).unwrap();
        assert_eq!(l.title, "Lanius (Battle)");
        assert_eq!(l.entries[0].id, track_id(Site::Bandcamp, "2650410135"));
        let m = map_track(Site::Bandcamp, &l.entries[0].id, &v, None, 1);
        assert_eq!(
            (m.track.title.as_str(), m.track.artists[0].name.as_str()),
            ("Lanius (Battle)", "Ben Prunty")
        );
        assert_eq!(m.album.title, "FTL: Advanced Edition Soundtrack");
        assert_eq!(
            (m.album.album_type, m.track.track_no, m.album.year),
            (AlbumType::Album, Some(1), Some(2014))
        );
        assert_eq!(
            m.cover_url.as_deref(),
            Some("https://f4.bcbits.com/img/a1270682128_5.jpg")
        );

        let album =
            parse_external("https://blazo.bandcamp.com/album/jazz-format-mixtape-vol-1").unwrap();
        let l = parse_listing(&album, &fixture("ytdlp_bc_album_flat.json")).unwrap();
        assert_eq!(
            (l.title.as_str(), l.kind_name(), l.entries.len()),
            ("Jazz Format Mixtape vol.1", "album", 3)
        );
        assert_eq!(
            l.entries[1].url,
            "https://blazo.bandcamp.com/track/kero-one-keep-it-alive-blazo-remix"
        );
        let v = fixture("ytdlp_bc_album_track.json");
        let m = map_track(Site::Bandcamp, &l.entries[1].id, &v, None, 2);
        assert_eq!(m.track.title, "Kero One - Keep It Alive (Blazo remix)");
        assert_eq!(m.track.artists[0].name, "Blazo");
        assert_eq!(
            (m.track.track_no, m.album.title.as_str()),
            (Some(2), "Jazz Format Mixtape vol.1")
        );
        // The same album id whether the track came from the album or its own page.
        let mut alone = v.clone();
        alone
            .as_object_mut()
            .unwrap()
            .retain(|k, _| !k.starts_with("playlist"));
        assert_eq!(
            map_track(Site::Bandcamp, &l.entries[1].id, &alone, None, 1)
                .album
                .id,
            m.album.id
        );
    }

    #[test]
    fn soundcloud_profile() {
        let link = parse_external("https://soundcloud.com/forss").unwrap();
        let v = serde_json::json!({"_type": "playlist", "id": "183", "title": "Forss (Tracks)", "entries": [
            {"ie_key": "Soundcloud", "id": "49438146", "title": "Lux Aeterna", "_type": "url", "url": "https://soundcloud.com/forss/lux-aeterna"}
        ]});
        let l = parse_listing(&link, &v).unwrap();
        assert_eq!(
            (
                l.title.as_str(),
                l.kind_name(),
                l.entries.len(),
                l.set_album.is_none()
            ),
            ("Forss", "artist", 1, true)
        );
    }

    #[test]
    fn empty_or_broken_listing() {
        let link = parse_external("https://soundcloud.com/u/sets/empty").unwrap();
        let v: Value = serde_json::json!({"_type": "playlist", "title": "Empty", "entries": []});
        assert!(parse_listing(&link, &v).is_err());
        let v: Value = serde_json::json!({"_type": "playlist", "entries": [{"id": "1", "url": "https://soundcloud.com/u/a"}, {"id": "1", "url": "https://soundcloud.com/u/a"}, {"title": "no id"}]});
        assert_eq!(parse_listing(&link, &v).unwrap().entries.len(), 1);
    }

    #[test]
    fn entry_links_must_stay_on_the_site() {
        let sc = Site::SoundCloud;
        assert_eq!(
            safe_url(sc, "https://api-v2.soundcloud.com/tracks/1?secret_token=s-x").as_deref(),
            Some("https://api-v2.soundcloud.com/tracks/1?secret_token=s-x")
        );
        for bad in [
            "http://soundcloud.com/u/a",
            "https://evil.com/soundcloud.com",
            "https://soundcloud.com.evil.com/u/a",
            "https://evilsoundcloud.com/u/a",
            "https://user@soundcloud.com/u/a",
            "https://soundcloud.com:8443/u/a",
            "file:///C:/Windows/win.ini",
            "--exec=calc",
        ] {
            assert_eq!(safe_url(sc, bad), None, "{bad}");
        }
        assert!(safe_url(Site::Bandcamp, "https://artist.bandcamp.com/track/x").is_some());
        assert!(safe_url(Site::Bandcamp, "https://soundcloud.com/u/a").is_none());

        let link = parse_external("https://soundcloud.com/u/sets/mix").unwrap();
        let v = serde_json::json!({"_type": "playlist", "title": "Mix", "entries": [
            {"id": "1", "url": "https://example.com/a"},
            {"id": "2", "url": "https://soundcloud.com/u/b"}
        ]});
        let l = parse_listing(&link, &v).unwrap();
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.entries[0].url, "https://soundcloud.com/u/b");

        let track = parse_external("https://soundcloud.com/u/b").unwrap();
        let v = serde_json::json!({"id": "2", "title": "B", "webpage_url": "https://example.com/b"});
        assert_eq!(parse_listing(&track, &v).unwrap().entries[0].url, "https://soundcloud.com/u/b");
    }

    #[test]
    fn downloaded_file_must_be_the_requested_one() {
        let root = std::env::temp_dir().join(format!("mixtape-ext-{}", std::process::id()));
        let lib = root.join("library");
        std::fs::create_dir_all(&lib).unwrap();
        let id = track_id(Site::SoundCloud, "1");
        let good = lib.join(format!("{id}.m4a"));
        std::fs::write(&good, b"x").unwrap();
        let other = lib.join("other.m4a");
        std::fs::write(&other, b"x").unwrap();
        let outside = root.join(format!("{id}.m4a"));
        std::fs::write(&outside, b"x").unwrap();

        assert!(library_file(&lib, &id, &good).is_some());
        assert!(library_file(&lib, &id, &lib.join("..").join("library").join(format!("{id}.m4a"))).is_some());
        assert!(library_file(&lib, &id, &other).is_none());
        assert!(library_file(&lib, &id, &outside).is_none());
        assert!(library_file(&lib, &id, &lib.join(format!("{id}.m4a.part"))).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn previews_are_refused() {
        let v = serde_json::json!({"format_id": "http_mp3_0_0_preview", "duration": 30.0});
        assert!(is_preview(&v, 29_753));
        let v = fixture("ytdlp_sc_track.json");
        assert!(!is_preview(&v, 213_886));
        assert!(
            is_preview(&v, 30_000),
            "a full-length track that came out short"
        );
        assert!(!is_preview(&fixture("ytdlp_bc_album_track.json"), 181_467));
    }

    #[test]
    fn error_lines() {
        let e = ytdlp_error("WARNING: x\nERROR: [soundcloud] 293: This track is not available\n");
        assert!(e.ends_with("This track is not available"), "{e}");
        let e = ytdlp_error("ERROR: Unsupported URL: https://x\n");
        assert!(e.ends_with("Unsupported URL: https://x"), "{e}");
    }
}
