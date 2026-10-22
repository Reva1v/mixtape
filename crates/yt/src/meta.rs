//! What a single video id is: title, artists, album, length and cover, without signing in.
//! Used where a track arrives as a bare link (the Android app's "paste a link").

use crate::api::{VideoKind, YtTrack};
use crate::innertube::InnerTube;
use crate::radio::parse_page;
use crate::{Result, YtError};
use mixtape_core::{ArtistRef, TrackId, tr};
use serde_json::{Value, json};

/// A track and what came with it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackInfo {
    pub track: YtTrack,
    /// `responseContext.visitorData`, for [`crate::direct::DirectResolver::set_visitor_data`].
    pub visitor_data: Option<String>,
}

impl InnerTube {
    /// `next` for `id` (`WEB_REMIX`, works signed out): the first row of its queue.
    pub async fn track_info(&self, id: &TrackId) -> Result<TrackInfo> {
        let resp = self
            .call(
                "next",
                json!({ "videoId": id.as_str(), "isAudioOnly": true }),
            )
            .await?;
        parse_next(&resp, id).ok_or_else(|| {
            YtError::Unavailable(
                tr(
                    "Track not found on YouTube Music",
                    "Трек не знайдено в YouTube Music",
                    "трек не найден в YouTube Music",
                )
                .to_owned(),
            )
        })
    }
}

/// The row of `id` in a `next` answer; else the first row, kept under `id` (a clip's queue
/// may open with its audio version, but the user asked for this id).
pub fn parse_next(resp: &Value, id: &TrackId) -> Option<TrackInfo> {
    let page = parse_page(resp)?;
    let track = match page.tracks.iter().find(|t| t.id == *id) {
        Some(t) => t.clone(),
        None => YtTrack {
            id: id.clone(),
            ..page.tracks.into_iter().next()?
        },
    };
    Some(TrackInfo {
        track,
        visitor_data: visitor_data(resp),
    })
}

pub fn visitor_data(resp: &Value) -> Option<String> {
    resp["responseContext"]["visitorData"]
        .as_str()
        .map(str::to_owned)
}

/// A track from the `videoDetails` of a `player` answer, when `next` is not available.
pub fn from_video_details(player: &Value) -> Option<YtTrack> {
    let d = player.get("videoDetails")?;
    let id = TrackId::new(d["videoId"].as_str()?);
    let author = d["author"].as_str().unwrap_or_default();
    let artist = author.strip_suffix(" - Topic").unwrap_or(author).trim();
    Some(YtTrack {
        id,
        title: d["title"].as_str()?.to_owned(),
        artists: (!artist.is_empty())
            .then(|| ArtistRef {
                id: None,
                name: artist.to_owned(),
            })
            .into_iter()
            .collect(),
        album: None,
        duration_ms: d["lengthSeconds"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
            * 1000,
        thumbnail: d["thumbnail"]["thumbnails"]
            .as_array()
            .and_then(|t| t.last())
            .and_then(|t| t["url"].as_str())
            .map(str::to_owned),
        kind: VideoKind::Audio,
        track_no: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn next_gives_title_artists_album_and_visitor() {
        let info = parse_next(&fixture("next_ok.json"), &TrackId::new("lYBUbBu4W08")).unwrap();
        let t = info.track;
        assert_eq!(t.title, "veylen");
        assert_eq!(t.artists.len(), 1);
        assert_eq!(t.artists[0].name, "DannZeth");
        assert_eq!(t.album.as_ref().map(|a| a.title.as_str()), Some("veylen"));
        assert_eq!(t.duration_ms, 155_000);
        assert!(t.thumbnail.is_some());
        assert_eq!(
            info.visitor_data.as_deref(),
            Some("CgtWaXNpdG9yRGF0YQ%3D%3D")
        );
    }

    #[test]
    fn unknown_id_keeps_the_asked_id() {
        let info = parse_next(&fixture("next_ok.json"), &TrackId::new("AAAAAAAAAAA")).unwrap();
        assert_eq!(info.track.id.as_str(), "AAAAAAAAAAA");
        assert_eq!(info.track.title, "veylen");
    }

    #[test]
    fn video_details_strip_topic() {
        let t = from_video_details(&fixture("player_ok.json")).unwrap();
        assert_eq!(t.title, "Never Gonna Give You Up");
        assert_eq!(t.artists[0].name, "Rick Astley");
        assert_eq!(t.duration_ms, 213_000);
    }
}
