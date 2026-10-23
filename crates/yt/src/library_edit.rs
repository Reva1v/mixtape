//! Saving to the account's library: an album or someone else's playlist is saved with the
//! like button of its playlist (`like/like` / `like/removelike` with `{target:{playlistId}}`;
//! an album's is its `OLAK5uy_…` audio playlist), an artist by subscribing to its channel
//! (`subscription/subscribe` / `subscription/unsubscribe` with `{channelIds}`). All need
//! cookies; a guest answer is an auth error like any other signed-in call.

use crate::Result;
use crate::innertube::InnerTube;
use mixtape_core::ArtistId;
use serde_json::{Value, json};

/// Endpoint and body that saves (`saved`) or unsaves a playlist. `VL…` browse ids are taken
/// bare, as the like endpoints want them.
pub fn save_playlist_request(playlist_id: &str, saved: bool) -> (&'static str, Value) {
    let bare = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
    let endpoint = if saved { "like/like" } else { "like/removelike" };
    (endpoint, json!({ "target": { "playlistId": bare } }))
}

/// Endpoint and body that subscribes to (`subscribed`) or unsubscribes from an artist's channel.
pub fn subscribe_request(channel: &ArtistId, subscribed: bool) -> (&'static str, Value) {
    let endpoint = if subscribed { "subscription/subscribe" } else { "subscription/unsubscribe" };
    (endpoint, json!({ "channelIds": [channel.as_str()] }))
}

impl InnerTube {
    /// Saves a playlist (an album's `OLAK5uy_…` one too) to the library or takes it out.
    pub async fn set_playlist_saved(&self, playlist_id: &str, saved: bool) -> Result<()> {
        let (endpoint, body) = save_playlist_request(playlist_id, saved);
        self.call(endpoint, body).await.map(drop)
    }

    /// Subscribes to an artist or unsubscribes.
    pub async fn set_subscribed(&self, channel: &ArtistId, subscribed: bool) -> Result<()> {
        let (endpoint, body) = subscribe_request(channel, subscribed);
        self.call(endpoint, body).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_bodies() {
        let (endpoint, body) = save_playlist_request("VLPLabc", true);
        assert_eq!(endpoint, "like/like");
        assert_eq!(body["target"]["playlistId"], "PLabc");
        let (endpoint, body) = save_playlist_request("OLAK5uy_album", false);
        assert_eq!(endpoint, "like/removelike");
        assert_eq!(body["target"]["playlistId"], "OLAK5uy_album");
    }

    #[test]
    fn subscribe_bodies() {
        let (endpoint, body) = subscribe_request(&ArtistId::from("UCartist"), true);
        assert_eq!(endpoint, "subscription/subscribe");
        assert_eq!(body["channelIds"][0], "UCartist");
        assert_eq!(subscribe_request(&ArtistId::from("UCartist"), false).0, "subscription/unsubscribe");
    }
}
