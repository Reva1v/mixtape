//! The `YtMusic` boundary: what the rest of Mixtape may ask YouTube Music, in domain types.
//! `InnerTube` is the only implementation; sync code is generic so tests can fake it.

use crate::innertube::{InnerTube, MAX_PAGES};
use crate::parse::{self, at, find_all, find_first, runs, text};
use crate::{Result, YtError};
use mixtape_core::{AlbumId, AlbumRef, AlbumType, ArtistId, ArtistRef, PlaylistId, TrackId, tr};
use serde_json::{Value, json};
use std::future::Future;

/// What kind of upload a video is: the audio-only "art track" or a music video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoKind {
    Audio,
    Video,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YtTrack {
    pub id: TrackId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration_ms: u64,
    /// Small thumbnail URL as YouTube sent it; see `thumbs` for the large one.
    pub thumbnail: Option<String>,
    pub kind: VideoKind,
    pub track_no: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YtAlbumCard {
    pub id: AlbumId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub year: Option<u16>,
    pub album_type: Option<AlbumType>,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YtAlbum {
    pub card: YtAlbumCard,
    /// `OLAK5uy_…` playlist with the album's audio tracks.
    pub playlist_id: Option<String>,
    pub tracks: Vec<YtTrack>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YtPlaylistCard {
    /// Browse id, `VL…`.
    pub id: PlaylistId,
    pub title: String,
    pub author: Option<String>,
    pub track_count: Option<u32>,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YtPlaylist {
    pub card: YtPlaylistCard,
    pub tracks: Vec<YtTrack>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YtArtistCard {
    pub id: ArtistId,
    pub name: String,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YtArtist {
    pub card: YtArtistCard,
    pub top_tracks: Vec<YtTrack>,
    /// Albums, EPs and singles, newest first as YouTube lists them.
    pub discography: Vec<YtAlbumCard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub handle: Option<String>,
    pub avatar: Option<String>,
}

pub trait YtMusic: Send + Sync {
    fn account(&self) -> impl Future<Output = Result<Account>> + Send;
    fn liked_tracks(&self) -> impl Future<Output = Result<Vec<YtTrack>>> + Send;
    fn library_playlists(&self) -> impl Future<Output = Result<Vec<YtPlaylistCard>>> + Send;
    fn library_albums(&self) -> impl Future<Output = Result<Vec<YtAlbumCard>>> + Send;
    /// Artists the user subscribed to.
    fn library_artists(&self) -> impl Future<Output = Result<Vec<YtArtistCard>>> + Send;
    fn playlist(&self, id: &PlaylistId) -> impl Future<Output = Result<YtPlaylist>> + Send;
    fn album(&self, id: &AlbumId) -> impl Future<Output = Result<YtAlbum>> + Send;
    fn artist(&self, id: &ArtistId) -> impl Future<Output = Result<YtArtist>> + Send;
    /// One song by video id, with album and artists, as the watch page shows it.
    fn track(&self, id: &TrackId) -> impl Future<Output = Result<YtTrack>> + Send;
}

impl InnerTube {
    /// Rows of a song list (playlist, likes) across every continuation page.
    async fn all_rows(&self, first: &Value) -> Result<Vec<YtTrack>> {
        let (items, mut token) = parse::shelf_items(first);
        let mut tracks: Vec<YtTrack> = items.iter().filter_map(|i| row_track(i)).collect();
        for _ in 0..MAX_PAGES {
            let Some(t) = token.take() else { break };
            let page = self.browse_continuation(&t).await?;
            let (items, next) = parse::continuation_items(&page);
            tracks.extend(items.iter().filter_map(|i| row_track(i)));
            token = next;
        }
        Ok(tracks)
    }

    async fn all_grid<T>(
        &self,
        first: &Value,
        map: impl Fn(&Value) -> Option<T>,
    ) -> Result<Vec<T>> {
        let (items, mut token) = parse::grid_items(first);
        let mut out: Vec<T> = items
            .iter()
            .filter_map(|i| i.get("musicTwoRowItemRenderer").and_then(&map))
            .collect();
        for _ in 0..MAX_PAGES {
            let Some(t) = token.take() else { break };
            let page = self.browse_continuation(&t).await?;
            let (items, next) = parse::continuation_items(&page);
            out.extend(
                items
                    .iter()
                    .filter_map(|i| i.get("musicTwoRowItemRenderer").and_then(&map)),
            );
            token = next;
        }
        Ok(out)
    }
}

fn row_track(item: &Value) -> Option<YtTrack> {
    parse::list_item_track(item.get("musicResponsiveListItemRenderer")?)
}

impl YtMusic for InnerTube {
    async fn account(&self) -> Result<Account> {
        let resp = self.call("account/account_menu", json!({})).await?;
        let header = find_first(&resp, "activeAccountHeaderRenderer").ok_or(YtError::Auth)?;
        Ok(Account {
            name: text(header.get("accountName")),
            handle: header
                .get("channelHandle")
                .map(|h| text(Some(h)))
                .filter(|s| !s.is_empty()),
            avatar: header.get("accountPhoto").and_then(parse::thumbnail),
        })
    }

    async fn liked_tracks(&self) -> Result<Vec<YtTrack>> {
        let page = self.browse_signed_in("VLLM").await?;
        self.all_rows(&page).await
    }

    async fn library_playlists(&self) -> Result<Vec<YtPlaylistCard>> {
        let page = self.browse_signed_in("FEmusic_liked_playlists").await?;
        let mut out = self.all_grid(&page, parse::two_row_playlist).await?;
        // Likes are a source of their own; "Episodes for later" is not music.
        out.retain(|p| p.id.as_str() != "VLLM" && p.id.as_str() != "VLSE");
        Ok(out)
    }

    async fn library_albums(&self) -> Result<Vec<YtAlbumCard>> {
        let page = self.browse_signed_in("FEmusic_liked_albums").await?;
        self.all_grid(&page, parse::two_row_album).await
    }

    async fn library_artists(&self) -> Result<Vec<YtArtistCard>> {
        let page = self
            .browse_signed_in("FEmusic_library_corpus_artists")
            .await?;
        let (items, mut token) = parse::shelf_items(&page);
        let pick = |i: &&Value| {
            i.get("musicResponsiveListItemRenderer")
                .and_then(parse::list_item_artist)
        };
        let mut out: Vec<YtArtistCard> = items.iter().filter_map(pick).collect();
        for _ in 0..MAX_PAGES {
            let Some(t) = token.take() else { break };
            let page = self.browse_continuation(&t).await?;
            let (items, next) = parse::continuation_items(&page);
            out.extend(items.iter().filter_map(pick));
            token = next;
        }
        Ok(out)
    }

    async fn playlist(&self, id: &PlaylistId) -> Result<YtPlaylist> {
        let browse_id = if id.as_str().starts_with("VL") {
            id.0.clone()
        } else {
            format!("VL{}", id)
        };
        let page = self.browse(&browse_id).await?;
        let header = page_header(&page);
        let tracks = self.all_rows(&page).await?;
        let second = text(header.and_then(|h| h.get("secondSubtitle")));
        let card = YtPlaylistCard {
            id: PlaylistId::new(browse_id),
            title: text(header.and_then(|h| h.get("title"))),
            author: header
                .and_then(|h| h.get("facepile").or_else(|| h.get("straplineTextOne")))
                .map(|v| text(find_first(v, "text").or(Some(v))))
                .filter(|s| !s.is_empty()),
            track_count: parse::leading_number(&second).or(Some(tracks.len() as u32)),
            thumbnail: header
                .and_then(|h| h.get("thumbnail"))
                .and_then(parse::thumbnail),
        };
        if card.title.is_empty() && tracks.is_empty() {
            return Err(YtError::Unavailable(
                tr(
                    "Playlist not found or hidden",
                    "Плейлист не знайдено або приховано",
                    "плейлист не найден или скрыт",
                )
                .into(),
            ));
        }
        Ok(YtPlaylist { card, tracks })
    }

    async fn album(&self, id: &AlbumId) -> Result<YtAlbum> {
        let page = self.browse(id.as_str()).await?;
        let header = page_header(&page).ok_or_else(|| {
            YtError::Unavailable(
                tr("Album not found", "Альбом не знайдено", "альбом не найден").into(),
            )
        })?;
        let sub = runs(header.get("subtitle"));
        let artists = parse::artists_from_runs(runs(header.get("straplineTextOne")));
        let card = YtAlbumCard {
            id: id.clone(),
            title: text(header.get("title")),
            artists: artists.clone(),
            year: sub
                .iter()
                .find_map(|r| parse::year(r.get("text").and_then(Value::as_str).unwrap_or(""))),
            album_type: sub
                .first()
                .and_then(|r| parse::album_type(r.get("text").and_then(Value::as_str)?)),
            thumbnail: header.get("thumbnail").and_then(parse::thumbnail),
        };
        let album_ref = AlbumRef {
            id: id.clone(),
            title: card.title.clone(),
        };
        let mut tracks = self.all_rows(&page).await?;
        for t in &mut tracks {
            if t.artists.is_empty() {
                t.artists = artists.clone();
            }
            t.album.get_or_insert_with(|| album_ref.clone());
            if t.thumbnail.is_none() {
                t.thumbnail = card.thumbnail.clone();
            }
        }
        let playlist_id = find_all(&page, "playlistId")
            .into_iter()
            .filter_map(Value::as_str)
            .find(|p| p.starts_with("OLAK"))
            .map(str::to_owned);
        Ok(YtAlbum {
            card,
            playlist_id,
            tracks,
        })
    }

    async fn artist(&self, id: &ArtistId) -> Result<YtArtist> {
        let page = self.browse(id.as_str()).await?;
        let header = page
            .get("header")
            .and_then(|h| h.as_object()?.values().next())
            .ok_or_else(|| {
                YtError::Unavailable(
                    tr(
                        "Artist not found",
                        "Виконавця не знайдено",
                        "исполнитель не найден",
                    )
                    .into(),
                )
            })?;
        let card = YtArtistCard {
            id: id.clone(),
            name: text(header.get("title")),
            thumbnail: header.get("thumbnail").and_then(parse::thumbnail),
        };
        let top_tracks = parse::shelf_items(&page)
            .0
            .iter()
            .filter_map(|i| row_track(i))
            .collect();
        let mut discography: Vec<YtAlbumCard> = Vec::new();
        for shelf in find_all(&page, "musicCarouselShelfRenderer") {
            let items = shelf
                .get("contents")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let cards: Vec<YtAlbumCard> = items
                .iter()
                .filter_map(|i| {
                    i.get("musicTwoRowItemRenderer")
                        .and_then(parse::two_row_album)
                })
                .collect();
            if cards.is_empty() {
                continue;
            }
            // "See all" pages list only years; the shelf title ("Синглы и EP") gives the type.
            let shelf_type = parse::album_type(&text(
                shelf.get("header").and_then(|h| find_first(h, "title")),
            ));
            let typed = |mut a: YtAlbumCard| {
                a.album_type = a.album_type.or(shelf_type);
                a
            };
            // A full list sits behind "more" (`UC…` + params); the library shelf (`MPLA…`) is ours already.
            let more = shelf
                .get("header")
                .and_then(|h| find_first(h, "browseEndpoint"));
            let more =
                more.and_then(|b| Some((b.get("browseId")?.as_str()?, b.get("params")?.as_str()?)));
            match more {
                Some((bid, _)) if bid.starts_with("MPLA") => continue,
                Some((bid, params)) => {
                    let all = self.browse_params(bid, params).await?;
                    discography.extend(
                        self.all_grid(&all, parse::two_row_album)
                            .await?
                            .into_iter()
                            .map(typed),
                    );
                }
                None => discography.extend(cards.into_iter().map(typed)),
            }
        }
        let mut seen = std::collections::HashSet::new();
        discography.retain(|a| seen.insert(a.id.clone()));
        Ok(YtArtist {
            card,
            top_tracks,
            discography,
        })
    }

    async fn track(&self, id: &TrackId) -> Result<YtTrack> {
        let resp = self
            .call(
                "next",
                json!({ "videoId": id.as_str(), "isAudioOnly": true }),
            )
            .await?;
        let item = find_all(&resp, "playlistPanelVideoRenderer")
            .into_iter()
            .find(|v| v.get("videoId").and_then(Value::as_str) == Some(id.as_str()))
            .ok_or_else(|| {
                YtError::Unavailable(
                    tr("Track unavailable", "Трек недоступний", "трек недоступен").into(),
                )
            })?;
        parse::panel_track(item)
            .ok_or_else(|| YtError::Parse("next: playlistPanelVideoRenderer".into()))
    }
}

/// The header of an album or playlist page, wherever this layout version keeps it.
fn page_header(page: &Value) -> Option<&Value> {
    find_first(page, "musicResponsiveHeaderRenderer")
        .or_else(|| find_first(page, "musicDetailHeaderRenderer"))
        .or_else(|| at(page, &["header"]).and_then(|h| h.as_object()?.values().next()))
}
