//! Development helpers (T037): import a local folder of audio files as library tracks,
//! so playback can be tried before YouTube Music sync exists. Debug builds only: release
//! builds neither register the command nor read `MIXTAPE_DEV_IMPORT`.
#![cfg_attr(not(debug_assertions), allow(dead_code, unused_imports))]

use super::{CmdResult, blocking};
use crate::commands::library::emit_changed;
use crate::state::Session;
use mixtape_core::{AlbumId, AlbumRef, AlbumType, AppError, ArtistRef, ErrorKind, TrackId, tr, trf};
use mixtape_library::repo::{self, AlbumMeta, FileLocation, MediaFile, TrackMeta};
use mixtape_library::views::ViewKind;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};

const EXTS: [&str; 4] = ["webm", "m4a", "opus", "ogg"];

fn short_hash(s: &str) -> String {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:011x}", h.finish() & 0xfff_ffff_ffff)
}

/// A cover image next to the audio file: same name, .jpg / .png / .webp.
fn sibling_cover(file: &Path) -> Option<Vec<u8>> {
    ["jpg", "png", "webp"].iter().find_map(|ext| std::fs::read(file.with_extension(ext)).ok())
}

/// "Artist - Title" file names become artist and title; anything else is a title.
fn split_name(stem: &str) -> (Option<String>, String) {
    match stem.split_once(" - ") {
        Some((a, t)) => (Some(a.trim().to_owned()), t.trim().to_owned()),
        None => (None, stem.trim().to_owned()),
    }
}

fn audio_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            out.extend(audio_files(&path)?);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Registered in debug builds only (see `lib.rs`).
#[tauri::command]
pub async fn dev_import_folder(app: AppHandle, state: State<'_, Arc<Session>>, path: String) -> CmdResult<u32> {
    let st = state.inner().clone();
    let n = blocking(move || import_dir(&st, &path)).await?;
    emit_changed(&app, &[ViewKind::Tracks, ViewKind::Albums, ViewKind::Artists]);
    Ok(n)
}

/// Imports every audio file under `path`. Also run at startup in debug builds when
/// `MIXTAPE_DEV_IMPORT` is set, for screenshots and manual testing.
pub fn import_dir(st: &Session, path: &str) -> CmdResult<u32> {
    {
        let dir = PathBuf::from(path.trim().trim_matches('"'));
        let files = audio_files(&dir)
            .map_err(|e| {
                AppError::new(
                    ErrorKind::Invalid,
                    trf!("Couldn't read the folder: {e}", "Не вдалося прочитати теку: {e}", "Не удалось прочитать папку: {e}"),
                )
            })?;
        let mut imported = 0u32;
        for file in files {
            let Ok(info) = mixtape_audio::probe(&file) else { continue };
            let full = file.to_string_lossy().into_owned();
            let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or(tr("Untitled", "Без назви", "Без названия"));
            let (artist, title) = split_name(stem);
            let id = TrackId(format!("local{}", short_hash(&full)));
            let artists = vec![ArtistRef { id: None, name: artist.unwrap_or_else(|| tr("Unknown artist", "Невідомий виконавець", "Неизвестный исполнитель").into()) }];
            // Each file becomes a single with its own cover, which is what YouTube Music
            // gives for loose tracks too.
            let processed = sibling_cover(&file).and_then(|b| st.covers.process(id.as_str(), &b, mixtape_artwork::Detail::Full).ok());
            let cover_key = processed.as_ref().map(|_| id.0.clone());
            let album = AlbumMeta {
                id: AlbumId(format!("localalb{}", short_hash(&full))),
                title: title.clone(),
                artists: artists.clone(),
                year: None,
                album_type: AlbumType::Single,
                cover_key: cover_key.clone(),
                palette: processed.map(|p| p.palette).unwrap_or_default(),
                track_count: Some(1),
            };
            let meta = TrackMeta {
                id,
                title,
                artists,
                album: Some(AlbumRef { id: album.id.clone(), title: album.title.clone() }),
                track_no: Some(1),
                duration_ms: info.duration_ms,
                cover_key,
            };
            let media = MediaFile {
                track_id: meta.id.clone(),
                path: full,
                container: info.container.clone(),
                itag: None,
                size_bytes: std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0),
                sample_rate: Some(info.sample_rate),
                location: FileLocation::Library,
            };
            st.db.write(move |t| {
                repo::upsert_album(t, &album)?;
                repo::upsert_track(t, &meta)?;
                repo::put_media_file(t, &media)
            })?;
            imported += 1;
        }
        // Imported credits have names only; give them artist ids (no longer done at start).
        st.db.write(|t| mixtape_library::ytm::link_artists_by_name(t).map(drop))?;
        Ok(imported)
    }
}
