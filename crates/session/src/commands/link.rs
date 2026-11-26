//! Shared YouTube Music links (feature 004): reading one out of text, loading a linked track,
//! building the plain link to share, and the links the OS, the browser extension or a second
//! launch hand to the running app (`mixtape://…`).

use super::CmdResult;
use crate::commands::catalog::yt_error;
use crate::commands::player::PLAY_REQUESTS;
use crate::state::Session;
use mixtape_core::{AlbumId, AppError, ArtistId, ErrorKind, PlaylistId, Track, TrackId, tr};
use mixtape_library::repo;
use mixtape_sync::mixtape_link::{self, MixtapeLink};
use mixtape_sync::sources;
use mixtape_sync::url_import::{self, Link};
use mixtape_yt::api::YtMusic;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Manager, State};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use ts_rs::TS;

type St<'a> = State<'a, Arc<Session>>;

/// What a shared YouTube Music link points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum SharedKind {
    Track,
    Album,
    Playlist,
    Artist,
}

/// A YouTube Music link found in text: what it points at and its id.
///
/// The id is in the form the interface's routes take: a playlist keeps the `VL` browse prefix
/// (`VLPL…`, `VLOLAK5uy_…`), the way playlists from the catalog and the library carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SharedLink {
    pub kind: SharedKind,
    pub id: String,
}

impl From<&Link> for SharedLink {
    fn from(link: &Link) -> Self {
        match link {
            Link::Track(id) => Self {
                kind: SharedKind::Track,
                id: id.as_str().to_owned(),
            },
            Link::Album(id) => Self {
                kind: SharedKind::Album,
                id: id.as_str().to_owned(),
            },
            Link::Playlist(id) => {
                let id = id.as_str();
                let id = id.strip_prefix("VL").unwrap_or(id);
                Self {
                    kind: SharedKind::Playlist,
                    id: format!("VL{id}"),
                }
            }
            Link::Artist(id) => Self {
                kind: SharedKind::Artist,
                id: id.as_str().to_owned(),
            },
        }
    }
}

/// The track id of a YouTube Music `videoId`: exactly 11 characters. Local files
/// (`local0000000001`), SoundCloud (`sc` + 16 hex) and Bandcamp ids are longer.
fn is_ytm_track(id: &str) -> bool {
    id.len() == 11 && TrackId::is_valid(id)
}

/// The link of a kind and id from the interface; `Invalid` for what has no YouTube Music page
/// (local and imported tracks, `local:` playlists, anything that isn't an id).
fn link_of(kind: SharedKind, id: &str) -> CmdResult<Link> {
    let id = id.trim();
    let is_id = |s: &str| TrackId::is_valid(s);
    match kind {
        SharedKind::Track if is_ytm_track(id) => Ok(Link::Track(TrackId::new(id))),
        SharedKind::Album if is_id(id) && id.starts_with("MPREb_") => {
            Ok(Link::Album(AlbumId::new(id)))
        }
        SharedKind::Playlist if is_id(id) && !id.starts_with("local") => {
            let id = id.strip_prefix("VL").unwrap_or(id);
            Ok(Link::Playlist(PlaylistId::new(id)))
        }
        SharedKind::Artist if is_id(id) && id.starts_with("UC") => {
            Ok(Link::Artist(ArtistId::new(id)))
        }
        _ => Err(AppError::new(
            ErrorKind::Invalid,
            tr(
                "This item exists only in your library, there is no link to it",
                "Цей елемент є лише у вашій бібліотеці, посилання на нього немає",
                "Этот элемент есть только в вашей библиотеке, ссылки на него нет",
            ),
        )),
    }
}

/// The link in `text`, if there is one: the search box and the paste handler ask this before
/// anything goes to the network.
#[tauri::command]
pub fn link_parse(text: String) -> Option<SharedLink> {
    url_import::find_link(&text).as_ref().map(SharedLink::from)
}

/// The plain `music.youtube.com` link of an item, for «Поделиться».
#[tauri::command]
pub fn link_share_url(kind: SharedKind, id: String) -> CmdResult<String> {
    link_of(kind, &id).map(|l| url_import::share_url(&l))
}

/// The track of a link, stored in the library as a catalog (`remote`) track when it wasn't
/// there. No account needed. A track the library already has is used as it is, so a link to
/// a downloaded song works offline.
async fn load_track(st: &Arc<Session>, id: &TrackId) -> CmdResult<Track> {
    let ids = [id.clone()];
    if let Some(t) = st
        .db
        .read(|c| repo::tracks_by_ids(c, &ids))?
        .into_iter()
        .next()
    {
        return Ok(t);
    }
    let found = st.yt.track(id).await.map_err(|e| yt_error(st, e))?;
    sources::store_tracks(st.yt.http(), &st.db, &st.covers, vec![found]).await?;
    st.db
        .read(|c| repo::tracks_by_ids(c, &ids))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::NotFound,
                tr("Track not found", "Трек не знайдено", "Трек не найден"),
            )
        })
}

/// The track a link points at, stored and ready to play or queue.
#[tauri::command]
pub async fn link_track(state: St<'_>, id: TrackId) -> CmdResult<Track> {
    let st = state.inner().clone();
    load_track(&st, &id).await
}

/// Opens the bundled browser extension's folder in Explorer, to load it into the browser.
#[tauri::command]
pub fn extension_open_folder(state: St<'_>) -> CmdResult<()> {
    let dir = extension_dir(state.app.path().resource_dir().ok());
    if !dir.is_dir() {
        return Err(AppError::new(
            ErrorKind::NotFound,
            tr(
                "The extension folder wasn't found",
                "Теку розширення не знайдено",
                "Папка расширения не найдена",
            ),
        ));
    }
    crate::commands::settings::open_in_explorer(&dir)
}

/// `link-catcher` in the resources of an installed app; in a debug build the repo's own
/// `tools/link-catcher`, which is what the bundle is made from.
fn extension_dir(resources: Option<std::path::PathBuf>) -> std::path::PathBuf {
    if cfg!(debug_assertions) {
        let repo =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/link-catcher");
        if repo.is_dir() {
            return repo;
        }
    }
    resources.map_or_else(|| "link-catcher".into(), |r| r.join("link-catcher"))
}

// ---------- links handed over to the running app ----------

/// What a handed-over link asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    /// Play the track next and jump to it; the queue behind it stays.
    Track(TrackId),
    /// Show a page; the interface routes.
    Route(SharedLink),
    /// Nothing the app understands.
    Failed,
}

/// What `url` asks for: a `mixtape://` link, else a plain YouTube Music link (a link given
/// on the command line works too). A `play` link plays its first track here.
fn decide(url: &str) -> Action {
    let link = match mixtape_link::parse(url) {
        Some(MixtapeLink::Play { ids, .. }) => ids.into_iter().next().map(Link::Track),
        Some(MixtapeLink::Open(link)) => Some(link),
        None => url_import::find_link(url),
    };
    match link {
        Some(Link::Track(id)) => Action::Track(id),
        Some(other) => Action::Route(SharedLink::from(&other)),
        None => Action::Failed,
    }
}

/// Links that arrived before the interface was ready wait here; after that they go straight on.
/// The sink is called with the lock held, so arrival order is kept even with several threads.
struct Inbox {
    state: Mutex<InboxState>,
}

struct InboxState {
    ready: bool,
    pending: VecDeque<String>,
}

impl Inbox {
    const fn new() -> Self {
        Self {
            state: Mutex::new(InboxState {
                ready: false,
                pending: VecDeque::new(),
            }),
        }
    }

    fn hand_over(&self, url: String, sink: &mut impl FnMut(String)) {
        let mut s = self.state.lock().expect("links inbox poisoned");
        if s.ready {
            sink(url);
        } else {
            s.pending.push_back(url);
        }
    }

    /// The interface is ready: everything waiting goes on, oldest first, and later links go
    /// straight on.
    fn set_ready(&self, sink: &mut impl FnMut(String)) {
        let mut s = self.state.lock().expect("links inbox poisoned");
        s.ready = true;
        while let Some(url) = s.pending.pop_front() {
            sink(url);
        }
    }
}

static INBOX: Inbox = Inbox::new();
/// Links are acted on one after another by one task, so the last one handed over is what plays.
static WORKER: OnceLock<UnboundedSender<String>> = OnceLock::new();

fn worker(st: &Arc<Session>) -> &'static UnboundedSender<String> {
    WORKER.get_or_init(|| {
        let (tx, mut rx) = unbounded_channel::<String>();
        let st = st.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(url) = rx.recv().await {
                act(&st, &url).await;
            }
        });
        tx
    })
}

/// A link arrived (a second launch, the command line, the browser extension). It is acted on
/// once the interface is ready; before that it waits its turn.
pub fn hand_over(st: &Arc<Session>, url: String) {
    let tx = worker(st);
    INBOX.hand_over(url, &mut |u| {
        let _ = tx.send(u);
    });
}

/// The interface is up: acts on the links that arrived meanwhile, in the order they came.
pub fn set_ready(st: &Arc<Session>) {
    let tx = worker(st);
    INBOX.set_ready(&mut |u| {
        let _ = tx.send(u);
    });
}

/// Whether a link is waiting for the interface; the window must come up for it.
pub fn has_pending() -> bool {
    !INBOX
        .state
        .lock()
        .expect("links inbox poisoned")
        .pending
        .is_empty()
}

fn failed(st: &Session, text: String) {
    crate::host::emit(&st.app, "link://failed", text);
}

async fn act(st: &Arc<Session>, url: &str) {
    match decide(url) {
        Action::Route(link) => crate::host::emit(&st.app, "link://route", link),
        Action::Track(id) => {
            if let Err(e) = play_linked(st, id).await {
                failed(st, e.to_string());
            }
        }
        Action::Failed => failed(
            st,
            tr(
                "Couldn't open the link",
                "Не вдалося відкрити посилання",
                "Не удалось открыть ссылку",
            )
            .to_string(),
        ),
    }
}

/// `link_track`, then the track goes next in the queue and plays at once; what was queued
/// stays. An idle player just starts it. A newer play request meanwhile wins.
async fn play_linked(st: &Arc<Session>, id: TrackId) -> CmdResult<()> {
    let req = PLAY_REQUESTS.begin();
    let track = load_track(st, &id).await?;
    if !PLAY_REQUESTS.is_latest(req) {
        return Ok(());
    }
    let st = st.clone();
    super::blocking(move || {
        let player = st.player()?;
        let had_current = player.edit_queue(|q| {
            let had = q.current().is_some();
            q.play_next(vec![track.id]);
            had
        });
        // With nothing current the queue edit already started it.
        if had_current {
            player.next();
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    const VID: &str = "dQw4w9WgXcQ";
    const PL: &str = "PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI";
    const OLAK: &str = "OLAK5uy_kvwmmSDRVnqzAJ6bxFXAiDpcBsAH9FYvA";
    const ALBUM: &str = "MPREb_4pL8gzRtw1p";
    const UC: &str = "UCuAXFkgsw1L7xaCfnd5JJOw";

    fn shared(kind: SharedKind, id: &str) -> SharedLink {
        SharedLink {
            kind,
            id: id.into(),
        }
    }

    #[test]
    fn shared_link_maps_every_kind() {
        let f = |l: Link| SharedLink::from(&l);
        assert_eq!(
            f(Link::Track(TrackId::new(VID))),
            shared(SharedKind::Track, VID)
        );
        assert_eq!(
            f(Link::Album(AlbumId::new(ALBUM))),
            shared(SharedKind::Album, ALBUM)
        );
        assert_eq!(
            f(Link::Artist(ArtistId::new(UC))),
            shared(SharedKind::Artist, UC)
        );
        assert_eq!(
            f(Link::Playlist(PlaylistId::new(PL))),
            shared(SharedKind::Playlist, &format!("VL{PL}"))
        );
    }

    #[test]
    fn playlist_ids_get_vl_once_and_album_playlists_stay_playlists() {
        let f = |id: &str| SharedLink::from(&Link::Playlist(PlaylistId::new(id)));
        assert_eq!(f(&format!("VL{PL}")).id, format!("VL{PL}"));
        let album_playlist = f(OLAK);
        assert_eq!(album_playlist.kind, SharedKind::Playlist);
        assert_eq!(album_playlist.id, format!("VL{OLAK}"));
    }

    #[test]
    fn shared_link_serializes_as_the_interface_expects() {
        let json = serde_json::to_string(&shared(SharedKind::Playlist, "VLx")).unwrap();
        assert_eq!(json, r#"{"kind":"playlist","id":"VLx"}"#);
        for (k, s) in [
            (SharedKind::Track, "track"),
            (SharedKind::Album, "album"),
            (SharedKind::Playlist, "playlist"),
            (SharedKind::Artist, "artist"),
        ] {
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{s}\""));
            assert_eq!(
                serde_json::from_str::<SharedKind>(&format!("\"{s}\"")).unwrap(),
                k
            );
        }
    }

    #[test]
    fn link_parse_finds_links_in_text() {
        let url = format!("https://music.youtube.com/watch?v={VID}&si=abc");
        assert_eq!(
            link_parse(format!("listen: {url} it's good")),
            Some(shared(SharedKind::Track, VID))
        );
        assert_eq!(
            link_parse(format!("https://music.youtube.com/playlist?list={PL}")),
            Some(shared(SharedKind::Playlist, &format!("VL{PL}")))
        );
        assert_eq!(
            link_parse(format!("https://music.youtube.com/playlist?list={OLAK}")).map(|l| l.kind),
            Some(SharedKind::Playlist)
        );
        assert_eq!(
            link_parse(format!("https://music.youtube.com/browse/{ALBUM}")),
            Some(shared(SharedKind::Album, ALBUM))
        );
        assert_eq!(
            link_parse(format!("https://music.youtube.com/channel/{UC}")),
            Some(shared(SharedKind::Artist, UC))
        );
    }

    #[test]
    fn link_parse_is_none_without_a_link() {
        assert_eq!(link_parse("just some words here".into()), None);
        assert_eq!(link_parse(String::new()), None);
        assert_eq!(
            link_parse("https://example.com/watch?v=dQw4w9WgXcQ".into()),
            None
        );
    }

    #[test]
    fn share_url_per_kind() {
        let u = |k, id: &str| link_share_url(k, id.into()).unwrap();
        assert_eq!(
            u(SharedKind::Track, VID),
            format!("https://music.youtube.com/watch?v={VID}")
        );
        assert_eq!(
            u(SharedKind::Album, ALBUM),
            format!("https://music.youtube.com/browse/{ALBUM}")
        );
        assert_eq!(
            u(SharedKind::Artist, UC),
            format!("https://music.youtube.com/channel/{UC}")
        );
        // With and without the browse prefix, and for an album's playlist.
        let pl = format!("https://music.youtube.com/playlist?list={PL}");
        assert_eq!(u(SharedKind::Playlist, PL), pl);
        assert_eq!(u(SharedKind::Playlist, &format!("VL{PL}")), pl);
        assert_eq!(
            u(SharedKind::Playlist, OLAK),
            format!("https://music.youtube.com/playlist?list={OLAK}")
        );
    }

    #[test]
    fn share_url_round_trips_through_the_parser() {
        for l in [
            SharedLink::from(&Link::Track(TrackId::new(VID))),
            SharedLink::from(&Link::Playlist(PlaylistId::new(PL))),
            SharedLink::from(&Link::Album(AlbumId::new(ALBUM))),
            SharedLink::from(&Link::Artist(ArtistId::new(UC))),
        ] {
            let url = link_share_url(l.kind, l.id.clone()).unwrap();
            assert_eq!(link_parse(url), Some(l));
        }
    }

    #[test]
    fn share_url_rejects_what_has_no_page() {
        let bad = [
            (SharedKind::Track, "local0000000001"),
            (SharedKind::Track, "sc0123456789abcdef"),
            (SharedKind::Track, "bc12345"),
            (SharedKind::Track, ""),
            (SharedKind::Track, "short"),
            (SharedKind::Track, "dQw4w9WgXc!"),
            (
                SharedKind::Playlist,
                "local:7d444840-9dc0-11d1-b245-5ffdce74fad2",
            ),
            (SharedKind::Playlist, ""),
            (SharedKind::Playlist, "local0000000001"),
            (SharedKind::Playlist, "PLabc def!"),
            (SharedKind::Album, "MPREb_abc def!"),
            (SharedKind::Artist, "UCabc def!"),
            (SharedKind::Album, "local0000000001"),
            (SharedKind::Album, "dQw4w9WgXcQ"),
            (SharedKind::Artist, "local-artist"),
            (SharedKind::Artist, "not a channel"),
        ];
        for (kind, id) in bad {
            let e = link_share_url(kind, id.into()).unwrap_err();
            assert_eq!(e.kind, ErrorKind::Invalid, "{kind:?} {id}");
        }
    }

    #[test]
    fn link_of_trims_and_drops_the_browse_prefix() {
        assert_eq!(
            link_of(SharedKind::Track, &format!("  {VID}\n")).unwrap(),
            Link::Track(TrackId::new(VID))
        );
        assert_eq!(
            link_of(SharedKind::Playlist, &format!(" VL{PL} ")).unwrap(),
            Link::Playlist(PlaylistId::new(PL))
        );
        assert_eq!(
            link_of(SharedKind::Playlist, PL).unwrap(),
            Link::Playlist(PlaylistId::new(PL))
        );
    }

    // ---- links handed over ----

    fn feed(inbox: &Inbox, url: &str, out: &mut Vec<String>) {
        inbox.hand_over(url.into(), &mut |u| out.push(u));
    }

    #[test]
    fn nothing_runs_before_the_interface_is_ready() {
        let inbox = Inbox::new();
        let mut out = vec![];
        feed(&inbox, "a", &mut out);
        feed(&inbox, "b", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn ready_drains_in_arrival_order_then_runs_at_once() {
        let inbox = Inbox::new();
        let mut out = vec![];
        for u in ["a", "b", "c"] {
            feed(&inbox, u, &mut out);
        }
        inbox.set_ready(&mut |u| out.push(u));
        assert_eq!(out, ["a", "b", "c"]);
        feed(&inbox, "d", &mut out);
        assert_eq!(out, ["a", "b", "c", "d"]);
        // A second `ready` (the page reloaded) neither repeats nor loses anything.
        inbox.set_ready(&mut |u| out.push(u));
        assert_eq!(out.len(), 4);
    }

    /// The process-wide `INBOX` behind `has_pending`: the only test that touches it.
    #[test]
    fn has_pending_follows_the_global_inbox() {
        assert!(!has_pending());
        INBOX.hand_over("a".into(), &mut |_| {});
        assert!(has_pending());
        let mut out = vec![];
        INBOX.set_ready(&mut |u| out.push(u));
        assert_eq!(out, ["a"]);
        assert!(!has_pending());
    }

    #[test]
    fn ready_with_nothing_waiting_is_fine() {
        let inbox = Inbox::new();
        let mut out = vec![];
        inbox.set_ready(&mut |u| out.push(u));
        assert!(out.is_empty());
    }

    #[test]
    fn decide_track_links() {
        let want = Action::Track(TrackId::new(VID));
        let open = |scheme: &str, host: &str| {
            format!("{scheme}://{host}?url=https%3A%2F%2Fmusic.youtube.com%2Fwatch%3Fv%3D{VID}")
        };
        assert_eq!(decide(&open("mixtape", "open")), want);
        assert_eq!(decide(&open("MIXTAPE", "OPEN")), want);
        assert_eq!(
            decide(&format!("https://music.youtube.com/watch?v={VID}")),
            want
        );
    }

    #[test]
    fn decide_finds_a_plain_link_inside_text() {
        assert_eq!(
            decide(&format!(
                "listen https://music.youtube.com/watch?v={VID} now"
            )),
            Action::Track(TrackId::new(VID))
        );
    }

    #[test]
    fn decide_play_link_uses_the_first_id() {
        assert_eq!(
            decide(&format!("mixtape://play?ids={VID},9bZkp7q19f0&at=5000")),
            Action::Track(TrackId::new(VID))
        );
    }

    #[test]
    fn decide_pages_route() {
        assert_eq!(
            decide(&format!(
                "mixtape://open?url=https%3A%2F%2Fmusic.youtube.com%2Fbrowse%2F{ALBUM}"
            )),
            Action::Route(shared(SharedKind::Album, ALBUM))
        );
        assert_eq!(
            decide(&format!("https://music.youtube.com/playlist?list={PL}")),
            Action::Route(shared(SharedKind::Playlist, &format!("VL{PL}")))
        );
        assert_eq!(
            decide(&format!("https://music.youtube.com/channel/{UC}")),
            Action::Route(shared(SharedKind::Artist, UC))
        );
    }

    #[test]
    fn decide_garbage_fails() {
        for u in [
            "",
            "mixtape://",
            "mixtape://open",
            "mixtape://open?url=https%3A%2F%2Fexample.com%2F",
            "mixtape://play?ids=",
            "mixtape://nothing?x=1",
            "C:\\Music\\song.mp3",
            "--hidden",
        ] {
            assert_eq!(decide(u), Action::Failed, "{u:?}");
        }
    }

    #[test]
    fn extension_dir_uses_resources_in_release_and_the_repo_in_debug() {
        let res = std::path::PathBuf::from("C:/app");
        let dir = extension_dir(Some(res.clone()));
        if cfg!(debug_assertions) {
            assert!(dir.ends_with("tools/link-catcher"), "{dir:?}");
        } else {
            assert_eq!(dir, res.join("link-catcher"));
        }
        assert!(extension_dir(None).ends_with("link-catcher"));
    }
}
