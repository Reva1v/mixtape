//! Changing the account's own playlists: add a song (`browse/edit_playlist`,
//! `ACTION_ADD_VIDEO`), remove one (`ACTION_REMOVE_VIDEO`, addressed by the row's
//! `setVideoId`) and create a playlist (`playlist/create`). All need cookies; a guest
//! answer is an auth error like any other signed-in call.

use crate::innertube::{InnerTube, MAX_PAGES};
use crate::parse;
use crate::{Result, YtError};
use mixtape_core::{PlaylistId, TrackId, tr};
use serde_json::{Value, json};

/// The bare playlist id the edit endpoints want (`VLPL…` → `PL…`).
fn bare(id: &PlaylistId) -> &str {
    id.as_str().strip_prefix("VL").unwrap_or(id.as_str())
}

/// `browse/edit_playlist` body that appends `videos` to `playlist`.
pub fn add_body(playlist: &PlaylistId, videos: &[TrackId]) -> Value {
    let actions: Vec<Value> =
        videos.iter().map(|v| json!({ "action": "ACTION_ADD_VIDEO", "addedVideoId": v.as_str() })).collect();
    json!({ "playlistId": bare(playlist), "actions": actions })
}

/// `browse/edit_playlist` body that removes one row (`set_video_id`) of `video_id` from `playlist`.
pub fn remove_body(playlist: &PlaylistId, video_id: &TrackId, set_video_id: &str) -> Value {
    json!({
        "playlistId": bare(playlist),
        "actions": [{ "action": "ACTION_REMOVE_VIDEO", "setVideoId": set_video_id, "removedVideoId": video_id.as_str() }],
    })
}

/// `(videoId, playlistSetVideoId)` of each playlist row, in order; rows without both are skipped.
pub fn set_id_rows(items: &[&Value]) -> Vec<(TrackId, String)> {
    items
        .iter()
        .filter_map(|i| {
            let data = parse::at(i, &["musicResponsiveListItemRenderer", "playlistItemData"])?;
            let video = data.get("videoId")?.as_str()?;
            let set = data.get("playlistSetVideoId")?.as_str()?;
            Some((TrackId::new(video), set.to_owned()))
        })
        .collect()
}

/// `playlist/create` body: a private playlist named `title` holding `videos`.
pub fn create_body(title: &str, videos: &[TrackId]) -> Value {
    let ids: Vec<&str> = videos.iter().map(TrackId::as_str).collect();
    json!({ "title": title, "privacyStatus": "PRIVATE", "videoIds": ids })
}

/// The new playlist's id (as the library stores it, with `VL`) from a create answer.
pub fn created_id(resp: &Value) -> Option<PlaylistId> {
    let id = resp.get("playlistId")?.as_str()?;
    Some(PlaylistId::new(format!("VL{id}")))
}

/// `STATUS_SUCCEEDED` unless YouTube said otherwise; an answer without a status counts as done.
fn succeeded(resp: &Value) -> bool {
    resp.get("status").and_then(Value::as_str).is_none_or(|s| s == "STATUS_SUCCEEDED")
}

impl InnerTube {
    /// Appends songs to one of the account's playlists.
    pub async fn playlist_add(&self, playlist: &PlaylistId, videos: &[TrackId]) -> Result<()> {
        let resp = self.call("browse/edit_playlist", add_body(playlist, videos)).await?;
        if succeeded(&resp) {
            Ok(())
        } else {
            Err(YtError::Parse(
                tr("YouTube Music didn't change the playlist", "YouTube Music не змінив плейлист", "YouTube Music не изменил плейлист")
                    .into(),
            ))
        }
    }

    /// Removes one row (`set_video_id`, from [`Self::playlist_set_ids`]) of `video_id`.
    pub async fn playlist_remove(&self, playlist: &PlaylistId, video_id: &TrackId, set_video_id: &str) -> Result<()> {
        let resp = self.call("browse/edit_playlist", remove_body(playlist, video_id, set_video_id)).await?;
        if succeeded(&resp) {
            Ok(())
        } else {
            Err(YtError::Parse(
                tr("YouTube Music didn't change the playlist", "YouTube Music не змінив плейлист", "YouTube Music не изменил плейлист")
                    .into(),
            ))
        }
    }

    /// The playlist's rows in order with their `setVideoId`, across every continuation page.
    /// Signed in: a guest answer is `Auth`.
    pub async fn playlist_set_ids(&self, playlist: &PlaylistId) -> Result<Vec<(TrackId, String)>> {
        let page = self.browse_signed_in(&format!("VL{}", bare(playlist))).await?;
        let (items, mut token) = parse::shelf_items(&page);
        let mut rows = set_id_rows(&items);
        for _ in 0..MAX_PAGES {
            let Some(t) = token.take() else { break };
            let page = self.browse_continuation(&t).await?;
            let (items, next) = parse::continuation_items(&page);
            rows.extend(set_id_rows(&items));
            token = next;
        }
        Ok(rows)
    }

    /// Creates a private playlist in the account; returns its id.
    pub async fn playlist_create(&self, title: &str, videos: &[TrackId]) -> Result<PlaylistId> {
        let resp = self.call("playlist/create", create_body(title, videos)).await?;
        created_id(&resp).ok_or_else(|| {
            YtError::Parse(
                tr("YouTube Music didn't create the playlist", "YouTube Music не створив плейлист", "YouTube Music не создал плейлист")
                    .into(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_and_answers() {
        let body = add_body(&PlaylistId::new("VLPLabc"), &[TrackId::new("vid00000001")]);
        assert_eq!(body["playlistId"], "PLabc");
        assert_eq!(body["actions"][0]["action"], "ACTION_ADD_VIDEO");
        assert_eq!(body["actions"][0]["addedVideoId"], "vid00000001");

        let body = create_body("Мой", &[TrackId::new("vid00000001")]);
        assert_eq!(body["privacyStatus"], "PRIVATE");
        assert_eq!(body["videoIds"][0], "vid00000001");

        assert_eq!(created_id(&json!({ "playlistId": "PLnew" })), Some(PlaylistId::new("VLPLnew")));
        assert_eq!(created_id(&json!({})), None);
        assert!(succeeded(&json!({ "status": "STATUS_SUCCEEDED" })));
        assert!(succeeded(&json!({})));
        assert!(!succeeded(&json!({ "status": "STATUS_FAILED" })));
    }

    #[test]
    fn remove_body_shape() {
        let body = remove_body(&PlaylistId::new("VLPLabc"), &TrackId::new("vid00000001"), "SET1");
        assert_eq!(body["playlistId"], "PLabc");
        assert_eq!(body["actions"][0]["action"], "ACTION_REMOVE_VIDEO");
        assert_eq!(body["actions"][0]["setVideoId"], "SET1");
        assert_eq!(body["actions"][0]["removedVideoId"], "vid00000001");
        assert_eq!(body["actions"].as_array().map(Vec::len), Some(1));
    }

    fn row(video: &str, set: &str) -> Value {
        json!({ "musicResponsiveListItemRenderer": { "playlistItemData": { "videoId": video, "playlistSetVideoId": set } } })
    }

    #[test]
    fn set_ids_from_first_page_and_continuation() {
        let page = json!({ "contents": { "musicPlaylistShelfRenderer": { "contents": [
            row("vidA0000001", "S1"),
            { "musicResponsiveListItemRenderer": { "playlistItemData": { "videoId": "vidNoSet001" } } },
            row("vidB0000001", "S2"),
            row("vidA0000001", "S3"),
            { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "TOK" } } } },
        ] } } });
        let (items, token) = parse::shelf_items(&page);
        assert_eq!(token.as_deref(), Some("TOK"));
        let rows = set_id_rows(&items);
        let got: Vec<(&str, &str)> = rows.iter().map(|(v, s)| (v.as_str(), s.as_str())).collect();
        assert_eq!(got, vec![("vidA0000001", "S1"), ("vidB0000001", "S2"), ("vidA0000001", "S3")]);

        let cont = json!({ "onResponseReceivedActions": [{ "appendContinuationItemsAction": { "continuationItems": [
            row("vidC0000001", "S4"),
        ] } }] });
        let (items, next) = parse::continuation_items(&cont);
        assert_eq!(next, None);
        let rows = set_id_rows(&items);
        assert_eq!(rows, vec![(TrackId::new("vidC0000001"), "S4".to_owned())]);
    }
}
