//! The Mixtape app's side of the hand-off channel (contracts/handoff.md, sequences 1, 2, 4
//! and 7): it serves `-app` for the island, claims the playback from `-island` at start,
//! runs the island's buttons (`cmd`), passes library changes both ways and says `bye` with
//! its state when it quits (not to the tray).

use super::{Command, Handoff, Link, LinkError, LinkRx, LinkTx, Msg, Server, Side, TIMEOUT, keep_new, pipe_name};
use crate::commands::player::QueuePosition;
use crate::host::HandoffEvent;
use crate::state::Session;
use mixtape_core::{AppError, ErrorKind, TrackId};
use mixtape_library::views::ViewKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

/// The server and the one live link; managed by [`start`].
#[derive(Default)]
pub struct AppChannel {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    server: Option<Server>,
    link: Option<Current>,
}

struct Current {
    id: u64,
    opened_by: Side,
    tx: LinkTx,
}

static NEXT_LINK: AtomicU64 = AtomicU64::new(1);

fn lock(ch: &AppChannel) -> std::sync::MutexGuard<'_, Inner> {
    ch.inner.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the channel: the `-app` server, then a `claim` to the island if it runs.
pub fn start(st: &Arc<Session>) {
    st.app.manage(AppChannel::default());
    let rt = tauri::async_runtime::handle().inner().clone();
    match pipe_name(Side::App) {
        Ok(name) => {
            let s = st.clone();
            match Server::serve(&rt, &name, Side::App, move |link| adopt(&s, link)) {
                Ok(server) => lock(&st.app.state::<AppChannel>()).server = Some(server),
                Err(e) => tracing::warn!(error = %e, "hand-off: the app's pipe is not served"),
            }
        }
        Err(e) => tracing::warn!(error = %e, "hand-off: no pipe name"),
    }
    let st = st.clone();
    tauri::async_runtime::spawn(async move { claim(st).await });
}

/// Sequences 1 and 2: the island hands over what it plays, or answers `idle` and the app
/// keeps the snapshot it restored from the database.
async fn claim(st: Arc<Session>) {
    let name = match pipe_name(Side::Island) {
        Ok(n) => n,
        Err(_) => return,
    };
    let mut link = match Link::connect(&name, Side::App).await {
        Ok(l) => l,
        Err(LinkError::NoPeer) => return,
        Err(e) => {
            tracing::info!(error = %e, "hand-off: the island did not answer");
            return;
        }
    };
    match link.request(&Msg::Claim).await {
        Ok(Msg::Handoff { state }) => {
            let playing = state.playing;
            let res = take(&st, state).await;
            let ack = Msg::Ack { ok: res.is_ok(), reason: res.as_ref().err().map(|e| e.message.clone()) };
            if let Err(e) = link.send(&ack).await {
                tracing::info!(error = %e, "hand-off: ack not sent");
            }
            if res.is_ok() {
                st.host.on_handoff(&st.app, HandoffEvent::Took { playing });
            }
        }
        Ok(Msg::Idle) => {}
        Ok(other) => tracing::info!(?other, "hand-off: unexpected answer to claim"),
        Err(e) => {
            tracing::info!(error = %e, "hand-off: claim failed");
            return;
        }
    }
    adopt(&st, link);
}

async fn take(st: &Arc<Session>, state: Handoff) -> Result<(), AppError> {
    let st = st.clone();
    crate::commands::blocking(move || {
        let play = state.playing;
        st.handoff_in(state, play)
    })
    .await
}

/// Keeps `link` as the one live link, unless the one already there was opened by the app
/// and this one by the island (both started together: [`keep_new`]).
fn adopt(st: &Arc<Session>, link: Link) {
    let Some(ch) = st.app.try_state::<AppChannel>() else { return };
    let opened_by = link.opened_by();
    let peer = link.peer().clone();
    let id = NEXT_LINK.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = link.split();
    let old = {
        let mut inner = lock(&ch);
        if let Some(cur) = &inner.link
            && !keep_new(cur.opened_by, opened_by)
        {
            drop(inner);
            tauri::async_runtime::spawn(async move { tx.close().await });
            return;
        }
        inner.link.replace(Current { id, opened_by, tx })
    };
    if let Some(old) = old {
        tauri::async_runtime::spawn(async move { old.tx.close().await });
    }
    st.host.on_handoff(&st.app, HandoffEvent::Connected(peer));
    let st = st.clone();
    tauri::async_runtime::spawn(async move { listen(st, id, rx).await });
}

fn current_tx(app: &AppHandle, id: Option<u64>) -> Option<LinkTx> {
    let ch = app.try_state::<AppChannel>()?;
    let inner = lock(&ch);
    let cur = inner.link.as_ref()?;
    (id.is_none_or(|id| id == cur.id)).then(|| cur.tx.clone())
}

async fn listen(st: Arc<Session>, id: u64, mut rx: LinkRx) {
    loop {
        match rx.recv().await {
            Ok(Msg::Cmd { cmd }) => {
                let res = run(&st, cmd).await;
                let ack = Msg::Ack { ok: res.is_ok(), reason: res.err().map(|e| e.message) };
                if let Some(tx) = current_tx(&st.app, Some(id)) {
                    let _ = tx.send(&ack).await;
                }
            }
            Ok(Msg::Changed { kinds }) => crate::commands::library::emit_changed_local(&st.app, &kinds),
            // The island never claims from the app; nothing else needs an answer.
            Ok(Msg::Claim) => {
                if let Some(tx) = current_tx(&st.app, Some(id)) {
                    let _ = tx.send(&Msg::Idle).await;
                }
            }
            Ok(other) => tracing::debug!(?other, "hand-off: ignored"),
            Err(e) => {
                tracing::info!(error = %e, "hand-off: the island's link ended");
                let was_current = st.app.try_state::<AppChannel>().is_some_and(|ch| {
                    let mut inner = lock(&ch);
                    let cur = inner.link.as_ref().is_some_and(|c| c.id == id);
                    if cur {
                        inner.link = None;
                    }
                    cur
                });
                if was_current {
                    st.host.on_handoff(&st.app, HandoffEvent::PeerLeft { bye: false });
                }
                return;
            }
        }
    }
}

/// Runs an island button (sequence 7).
async fn run(st: &Arc<Session>, cmd: Command) -> Result<(), AppError> {
    let state = || st.app.state::<Arc<Session>>();
    match cmd {
        Command::Like { id } => crate::commands::mix::track_set_liked(state(), TrackId(id), true).await,
        Command::Unlike { id } => crate::commands::mix::track_set_liked(state(), TrackId(id), false).await,
        Command::StartMix { seed } => {
            let seed = match seed {
                Some(s) => TrackId(s),
                None => st.running_player().and_then(|p| p.state().track_id).ok_or_else(|| {
                    AppError::new(ErrorKind::NotFound, mixtape_core::tr("Nothing is playing", "Нічого не грає", "Ничего не играет"))
                })?,
            };
            crate::commands::mix::mix_start(state(), seed).await
        }
        Command::Enqueue { ids, next } => {
            let ids = ids.into_iter().map(TrackId).collect();
            let at = if next { QueuePosition::Next } else { QueuePosition::End };
            crate::commands::player::queue_add(state(), ids, at).await
        }
        Command::Jump { uid } => crate::commands::player::queue_jump(state(), uid).await,
        Command::Remove { uid } => crate::commands::player::queue_remove(state(), uid).await,
        Command::Move { uid, to } => crate::commands::player::queue_move(state(), uid, to as u32).await,
        Command::ShowWindow { screen } => {
            st.host.show_ui(&st.app, screen.as_deref());
            Ok(())
        }
    }
}

/// The library changed in the app: the island drops its cached views of these kinds.
pub fn changed(app: &AppHandle, kinds: &[ViewKind]) {
    let Some(tx) = current_tx(app, None) else { return };
    let msg = Msg::Changed { kinds: kinds.to_vec() };
    tauri::async_runtime::spawn(async move {
        let _ = tx.send(&msg).await;
    });
}

/// Sequence 4, when the app quits (not to the tray): pauses, sends `bye` with the state,
/// closes the link. Waits at most [`TIMEOUT`].
pub fn bye(app: &AppHandle) {
    let Some(tx) = app.try_state::<AppChannel>().and_then(|ch| lock(&ch).link.take()).map(|c| c.tx) else {
        return;
    };
    let state = app.try_state::<Arc<Session>>().and_then(|st| st.handoff_out());
    tauri::async_runtime::block_on(async move {
        let sent = tokio::time::timeout(TIMEOUT, async {
            let r = tx.send(&Msg::Bye { state }).await;
            tx.close().await;
            r
        })
        .await;
        if !matches!(sent, Ok(Ok(()))) {
            tracing::info!(?sent, "hand-off: bye not delivered");
        }
    });
}
