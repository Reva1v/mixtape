//! Radio ("Mix") on a seed track: the `next` queue `RDAMVM<id>`, its filter chips and
//! continuation pages, plus the like button. Parsing follows `parse.rs`: `Value` helpers,
//! a broken item is skipped instead of failing the page.

use crate::api::{VideoKind, YtTrack};
use crate::innertube::InnerTube;
use crate::parse::{self, find_all, find_first, text};
use crate::{Result, YtError};
use mixtape_core::TrackId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A filter over the mix ("Все", "Знакомое", "Популярное", genres…).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioChip {
    /// Stable across requests: the chip's playlist id (`params` change on every response),
    /// else its params, else its title.
    pub id: String,
    pub title: String,
    pub playlist_id: Option<String>,
    pub params: Option<String>,
    pub selected: bool,
}

/// One page of a mix. Chip responses and continuation pages carry no chips: keep the first ones.
#[derive(Debug, Clone, Default)]
pub struct RadioPage {
    pub tracks: Vec<YtTrack>,
    pub chips: Vec<RadioChip>,
    pub continuation: Option<String>,
    pub playlist_id: Option<String>,
}

/// `next` body for the first page of the mix on `seed`.
pub fn radio_body(seed: &TrackId) -> Value {
    json!({ "videoId": seed.as_str(), "playlistId": format!("RDAMVM{seed}"), "isAudioOnly": true })
}

/// `next` body for the mix filtered by `chip`. The chip's endpoint has no `videoId`: its
/// playlist and params already encode the seed; the seed is sent only for a chip without a playlist.
pub fn chip_body(seed: &TrackId, chip: &RadioChip) -> Value {
    let mut body = json!({ "isAudioOnly": true });
    match &chip.playlist_id {
        Some(pl) => body["playlistId"] = json!(pl),
        None => {
            body["videoId"] = json!(seed.as_str());
            body["playlistId"] = json!(format!("RDAMVM{seed}"));
        }
    }
    if let Some(params) = &chip.params {
        body["params"] = json!(params);
    }
    body
}

/// `next` body for the page after `continuation`.
pub fn more_body(playlist_id: &str, continuation: &str) -> Value {
    json!({ "playlistId": playlist_id, "continuation": continuation, "isAudioOnly": true })
}

/// Endpoint and body for the like button: `like/like` or `like/removelike`.
pub fn like_request(id: &TrackId, liked: bool) -> (&'static str, Value) {
    let endpoint = if liked {
        "like/like"
    } else {
        "like/removelike"
    };
    (endpoint, json!({ "target": { "videoId": id.as_str() } }))
}

/// A `next` response → mix page; `None` if it has no queue panel at all.
pub fn parse_page(resp: &Value) -> Option<RadioPage> {
    let panel = find_first(resp, "playlistPanelRenderer")
        .or_else(|| find_first(resp, "playlistPanelContinuation"))?;
    let items = panel
        .get("contents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let continuation = panel
        .get("continuations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|c| {
            c.get("nextRadioContinuationData")
                .or_else(|| c.get("nextContinuationData"))?
                .get("continuation")?
                .as_str()
        })
        .map(str::to_owned);
    Some(RadioPage {
        tracks: items.iter().filter_map(queue_item).collect(),
        chips: find_first(resp, "musicQueueRenderer")
            .map(chips)
            .unwrap_or_default(),
        continuation,
        playlist_id: panel
            .get("playlistId")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// A queue row: a plain `playlistPanelVideoRenderer`, or a wrapper holding the audio and the
/// clip of one song, from which the audio version (ATV) is taken.
pub fn queue_item(item: &Value) -> Option<YtTrack> {
    if let Some(r) = item.get("playlistPanelVideoRenderer") {
        return parse::panel_track(r);
    }
    let w = item.get("playlistPanelVideoWrapperRenderer")?;
    let primary = w
        .get("primaryRenderer")
        .and_then(|p| p.get("playlistPanelVideoRenderer"));
    let counterparts = w
        .get("counterpart")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let versions: Vec<YtTrack> = primary
        .into_iter()
        .chain(counterparts.filter_map(|c| find_first(c, "playlistPanelVideoRenderer")))
        .filter_map(parse::panel_track)
        .collect();
    let pick = versions
        .iter()
        .position(|t| t.kind == VideoKind::Audio)
        .unwrap_or(0);
    let mut track = versions.get(pick)?.clone();
    // The audio side of a clip-first wrapper often has bare artist names; borrow the links.
    if track.artists.iter().all(|a| a.id.is_none())
        && let Some(linked) = versions
            .iter()
            .find(|t| t.artists.iter().any(|a| a.id.is_some()))
    {
        track.artists = linked.artists.clone();
    }
    Some(track)
}

/// Filter chips of the queue header. Actions such as "Сохранить" (save the queue) have no
/// watch endpoint behind them and are skipped.
pub fn chips(queue: &Value) -> Vec<RadioChip> {
    find_all(queue, "chipCloudChipRenderer")
        .into_iter()
        .filter_map(|c| {
            let watch = find_first(c.get("navigationEndpoint")?, "watchEndpoint")?;
            let field = |k: &str| watch.get(k).and_then(Value::as_str).map(str::to_owned);
            let (playlist_id, params) = (field("playlistId"), field("params"));
            let title = text(c.get("text"));
            if title.is_empty() && playlist_id.is_none() {
                return None;
            }
            Some(RadioChip {
                id: playlist_id
                    .clone()
                    .or_else(|| params.clone())
                    .unwrap_or_else(|| title.clone()),
                title,
                playlist_id,
                params,
                selected: c
                    .get("isSelected")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

impl InnerTube {
    /// `next` {videoId: seed, playlistId: "RDAMVM"+seed} — first page of the mix (~48 tracks
    /// + continuation), with chips. The first track is usually the seed itself.
    pub async fn radio(&self, seed: &TrackId) -> Result<RadioPage> {
        let resp = self.call("next", radio_body(seed)).await?;
        let mut page = parse_page(&resp)
            .ok_or_else(|| YtError::Unavailable("микс для этого трека недоступен".into()))?;
        page.playlist_id
            .get_or_insert_with(|| format!("RDAMVM{seed}"));
        Ok(page)
    }

    /// Same mix filtered by a chip: `next` with the chip's playlistId/params.
    pub async fn radio_by_chip(&self, seed: &TrackId, chip: &RadioChip) -> Result<RadioPage> {
        let resp = self.call("next", chip_body(seed, chip)).await?;
        let mut page =
            parse_page(&resp).ok_or_else(|| YtError::Unavailable("микс недоступен".into()))?;
        if page.playlist_id.is_none() {
            page.playlist_id = chip
                .playlist_id
                .clone()
                .or_else(|| Some(format!("RDAMVM{seed}")));
        }
        Ok(page)
    }

    /// Next page: `next` {playlistId, continuation} → `nextRadioContinuationData`. A response
    /// without a queue is the end of the mix: an empty page without a continuation.
    pub async fn radio_more(&self, playlist_id: &str, continuation: &str) -> Result<RadioPage> {
        let resp = self
            .call("next", more_body(playlist_id, continuation))
            .await?;
        let mut page = parse_page(&resp).unwrap_or_default();
        page.playlist_id
            .get_or_insert_with(|| playlist_id.to_owned());
        Ok(page)
    }

    /// like/like or like/removelike with {target:{videoId}}.
    pub async fn set_like(&self, id: &TrackId, liked: bool) -> Result<()> {
        let (endpoint, body) = like_request(id, liked);
        self.call(endpoint, body).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn panel_video(id: &str, kind: &str, artist_id: Option<&str>) -> Value {
        let mut artist = json!({"text": "Rick Astley"});
        if let Some(a) = artist_id {
            artist["navigationEndpoint"] = json!({"browseEndpoint": {"browseId": a,
                "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}});
        }
        json!({"playlistPanelVideoRenderer": {
            "videoId": id, "title": {"runs": [{"text": "Never Gonna Give You Up"}]},
            "longBylineText": {"runs": [artist, {"text": " • "}, {"text": "1,8 млрд просмотров"}]},
            "lengthText": {"runs": [{"text": "3:34"}]},
            "navigationEndpoint": {"watchEndpoint": {"videoId": id,
                "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": kind}}}}}})
    }

    #[test]
    fn first_page() {
        let page = parse_page(&fixture("next_radio.json")).unwrap();
        assert!(page.tracks.len() >= 40, "{} tracks", page.tracks.len());
        assert_eq!(
            page.tracks[0].id.as_str(),
            "lYBUbBu4W08",
            "the seed comes first"
        );
        assert!(page.tracks.iter().all(|t| !t.title.is_empty()));
        assert!(page.continuation.is_some());
        assert_eq!(page.playlist_id.as_deref(), Some("RDAMVMlYBUbBu4W08"));
        // Every wrapper in this response is audio-first: no clip may leak into the queue.
        let raw = fixture("next_radio.json");
        let clips: Vec<&str> = find_all(&raw, "counterpartRenderer")
            .into_iter()
            .filter_map(|c| find_first(c, "videoId").and_then(Value::as_str))
            .collect();
        assert!(!clips.is_empty());
        assert!(page.tracks.iter().all(|t| !clips.contains(&t.id.as_str())));
        let seed = &page.tracks[0];
        assert_eq!(seed.kind, VideoKind::Audio);
        assert_eq!(seed.artists[0].name, "Rick Astley");
        assert!(seed.artists[0].id.is_some());
        assert!(seed.album.is_some());
    }

    #[test]
    fn chips_skip_the_save_action() {
        let raw = fixture("next_radio.json");
        let raw_titles: Vec<String> = find_all(&raw, "chipCloudChipRenderer")
            .into_iter()
            .map(|c| text(c.get("text")))
            .collect();
        assert!(
            raw_titles.iter().any(|t| t == "Сохранить"),
            "the response has the action"
        );
        let page = parse_page(&raw).unwrap();
        let titles: Vec<&str> = page.chips.iter().map(|c| c.title.as_str()).collect();
        assert!(!titles.contains(&"Сохранить"), "{titles:?}");
        for t in ["Все", "Знакомое", "Популярное"] {
            assert!(titles.contains(&t), "{t} missing in {titles:?}");
        }
        let selected: Vec<&str> = page
            .chips
            .iter()
            .filter(|c| c.selected)
            .map(|c| c.title.as_str())
            .collect();
        assert_eq!(selected, ["Все"]);
        assert!(
            page.chips
                .iter()
                .all(|c| c.playlist_id.is_some() && c.params.is_some())
        );
        let all = &page.chips[0];
        assert_eq!(all.id, "RDAMVMlYBUbBu4W08");
        let mut ids: Vec<&str> = page.chips.iter().map(|c| c.id.as_str()).collect();
        ids.dedup();
        assert_eq!(ids.len(), page.chips.len());
    }

    #[test]
    fn continuation_page() {
        let page = parse_page(&fixture("next_radio_more.json")).unwrap();
        assert!(page.tracks.len() >= 40, "{} tracks", page.tracks.len());
        assert!(page.continuation.is_some());
        assert!(page.chips.is_empty());
        assert_eq!(page.playlist_id.as_deref(), Some("RDAMVMlYBUbBu4W08"));
    }

    #[test]
    fn chip_page() {
        let page = parse_page(&fixture("next_radio_chip.json")).unwrap();
        assert!(page.tracks.len() >= 20, "{} tracks", page.tracks.len());
        assert!(page.continuation.is_some());
        assert!(
            page.chips.is_empty(),
            "only the save action is in a chip response"
        );
        assert!(
            page.playlist_id
                .as_deref()
                .is_some_and(|p| p.starts_with("RDAT"))
        );
    }

    #[test]
    fn wrapper_takes_audio_version() {
        // Clip first, audio as the counterpart (seed was a music video): audio wins and
        // borrows the artist link from the clip.
        let wrapper = json!({"playlistPanelVideoWrapperRenderer": {
            "primaryRenderer": panel_video("dQw4w9WgXcQ", "MUSIC_VIDEO_TYPE_OMV", Some("UCrick")),
            "counterpart": [{"counterpartRenderer": panel_video("lYBUbBu4W08", "MUSIC_VIDEO_TYPE_ATV", None)}]}});
        let t = queue_item(&wrapper).unwrap();
        assert_eq!(t.id.as_str(), "lYBUbBu4W08");
        assert_eq!(t.kind, VideoKind::Audio);
        assert_eq!(t.artists[0].id.as_ref().map(|a| a.as_str()), Some("UCrick"));
        assert_eq!(t.duration_ms, 214_000);
        // No audio at all: the primary stays.
        let clips = json!({"playlistPanelVideoWrapperRenderer": {
            "primaryRenderer": panel_video("aaaaaaaaaaa", "MUSIC_VIDEO_TYPE_OMV", None),
            "counterpart": [{"counterpartRenderer": panel_video("bbbbbbbbbbb", "MUSIC_VIDEO_TYPE_UGC", None)}]}});
        assert_eq!(queue_item(&clips).unwrap().id.as_str(), "aaaaaaaaaaa");
    }

    #[test]
    fn unplayable_items_are_skipped() {
        let resp = json!({"playlistPanelRenderer": {"playlistId": "RDAMVMx", "contents": [
            {"playlistPanelVideoRenderer": {"title": {"runs": [{"text": "Видео недоступно"}]}}},
            panel_video("lYBUbBu4W08", "MUSIC_VIDEO_TYPE_ATV", None),
            {"automixPreviewVideoRenderer": {}}
        ], "continuations": [{"nextContinuationData": {"continuation": "C2"}}]}});
        let page = parse_page(&resp).unwrap();
        assert_eq!(page.tracks.len(), 1);
        assert_eq!(page.continuation.as_deref(), Some("C2"));
        assert!(parse_page(&json!({"contents": {}})).is_none());
    }

    #[test]
    fn request_bodies() {
        let seed = TrackId::new("lYBUbBu4W08");
        let b = radio_body(&seed);
        assert_eq!(b["videoId"], "lYBUbBu4W08");
        assert_eq!(b["playlistId"], "RDAMVMlYBUbBu4W08");
        let chip = RadioChip {
            id: "RDATiYvlYBUbBu4W08".into(),
            title: "Знакомое".into(),
            playlist_id: Some("RDATiYvlYBUbBu4W08".into()),
            params: Some("kgQj".into()),
            selected: false,
        };
        let b = chip_body(&seed, &chip);
        assert_eq!(b["playlistId"], "RDATiYvlYBUbBu4W08");
        assert_eq!(b["params"], "kgQj");
        assert!(b.get("videoId").is_none());
        let bare = RadioChip {
            playlist_id: None,
            ..chip
        };
        assert_eq!(chip_body(&seed, &bare)["videoId"], "lYBUbBu4W08");
        let b = more_body("RDAMVMx", "CTOKEN");
        assert_eq!(
            (b["playlistId"].as_str(), b["continuation"].as_str()),
            (Some("RDAMVMx"), Some("CTOKEN"))
        );
    }

    #[test]
    fn like_bodies() {
        let id = TrackId::new("lYBUbBu4W08");
        let (endpoint, body) = like_request(&id, true);
        assert_eq!(endpoint, "like/like");
        assert_eq!(body, json!({"target": {"videoId": "lYBUbBu4W08"}}));
        assert_eq!(like_request(&id, false).0, "like/removelike");
    }
}
