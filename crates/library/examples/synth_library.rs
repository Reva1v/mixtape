//! Builds a synthetic library for the budget runs (quickstart.md): `n` downloaded tracks
//! with empty audio files, 900 artists, 1500 albums, a few playlists and play stats.
//! `cargo run --release -p mixtape-library --example synth_library -- <dir> 20000`
//! Then start the app with `MIXTAPE_DATA_DIR=<dir>`.

use mixtape_core::{AlbumId, AlbumRef, ArtistId, ArtistRef, TrackId, TrackState};
use mixtape_library::repo::{self, FileLocation, MediaFile, TrackMeta};
use mixtape_library::{DataPaths, Db};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let paths = DataPaths::at(std::path::PathBuf::from(&args[1]))?;
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let db = Db::open(&paths.db)?;
    let t = Instant::now();
    for i in 0..n {
        std::fs::write(paths.library.join(format!("syn{i:05}.webm")), b"")?;
    }
    db.write(move |t| {
        for i in 0..n {
            let id = TrackId(format!("syn{i:05}"));
            repo::upsert_track(
                t,
                &TrackMeta {
                    id: id.clone(),
                    title: format!("Song number {i} — песня"),
                    artists: vec![ArtistRef { id: Some(ArtistId(format!("UCsyn{}", i % 900))), name: format!("Artist {}", i % 900) }],
                    album: Some(AlbumRef { id: AlbumId(format!("MPsyn{}", i % 1500)), title: format!("Album {}", i % 1500) }),
                    track_no: Some((i / 1500) as u32 + 1),
                    duration_ms: 150_000 + (i as u64 * 7919) % 120_000,
                    cover_key: None,
                },
            )?;
            repo::put_media_file(
                t,
                &MediaFile {
                    track_id: id.clone(),
                    path: format!("library/syn{i:05}.webm"),
                    container: "webm".into(),
                    itag: Some(251),
                    size_bytes: 4_000_000,
                    sample_rate: Some(48_000),
                    location: FileLocation::Library,
                },
            )?;
            repo::set_state(t, &id, TrackState::Downloaded)?;
            if i % 7 == 0 {
                repo::record_play(t, &id, true)?;
            }
            if i % 11 == 0 {
                repo::set_liked(t, &id, true)?;
            }
        }
        Ok(())
    })?;
    println!("{n} tracks in {:?} at {}", t.elapsed(), paths.root.display());
    Ok(())
}
