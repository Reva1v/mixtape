//! Development helpers (T037): import a local folder of audio files as library tracks,
//! so playback can be tried before YouTube Music sync exists.

use super::{CmdResult, blocking};
use crate::commands::library::emit_changed;
use crate::state::AppState;
use mixtape_core::{AlbumId, AlbumRef, AppError, ArtistRef, ErrorKind, TrackId};
use mixtape_library::repo::{self, FileLocation, MediaFile, TrackMeta};
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

#[tauri::command]
pub async fn dev_import_folder(app: AppHandle, state: State<'_, Arc<AppState>>, path: String) -> CmdResult<u32> {
    let st = state.inner().clone();
    let n = blocking(move || {
        let dir = PathBuf::from(path.trim().trim_matches('"'));
        let files = audio_files(&dir)
            .map_err(|e| AppError::new(ErrorKind::Invalid, format!("Не удалось прочитать папку: {e}")))?;
        let mut imported = 0u32;
        for file in files {
            let Ok(info) = mixtape_audio::probe(&file) else { continue };
            let full = file.to_string_lossy().into_owned();
            let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("Без названия");
            let (artist, title) = split_name(stem);
            let folder = file.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()).unwrap_or("Папка");
            let parent = file.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let meta = TrackMeta {
                id: TrackId(format!("local{}", short_hash(&full))),
                title,
                artists: vec![ArtistRef { id: None, name: artist.unwrap_or_else(|| "Неизвестный исполнитель".into()) }],
                album: Some(AlbumRef { id: AlbumId(format!("localalb{}", short_hash(&parent))), title: folder.into() }),
                track_no: None,
                duration_ms: info.duration_ms,
                cover_key: None,
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
                repo::upsert_track(t, &meta)?;
                repo::put_media_file(t, &media)
            })?;
            imported += 1;
        }
        Ok(imported)
    })
    .await?;
    emit_changed(&app, &[ViewKind::Tracks, ViewKind::Albums, ViewKind::Artists]);
    Ok(n)
}
