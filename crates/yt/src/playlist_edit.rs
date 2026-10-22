//! Changing the account's own playlists: add a song (`browse/edit_playlist`,
//! `ACTION_ADD_VIDEO`) and create a playlist (`playlist/create`). Both need cookies; a guest
//! answer is an auth error like any other signed-in call.

use crate::innertube::InnerTube;
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
}
