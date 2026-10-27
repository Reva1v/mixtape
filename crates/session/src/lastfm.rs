//! Last.fm scrobbling: the connection made in settings (session key sealed with DPAPI in
//! `lastfm.bin`), "now playing" when a track starts, and scrobbles queued in
//! `scrobble_outbox` once a play counts (`mixtape_sync::lastfm::PlayTracker`).
//!
//! The outbox is flushed at start, when the network returns and after each new scrobble;
//! while rows are left after a failure a single retry timer backs off (1 min doubling to
//! 6 h). Nothing runs while nothing is queued, and nothing at all without a connection.

use crate::commands::{CmdResult, blocking};
use crate::state::Session as AppState;
use mixtape_core::{AppError, ErrorKind, Track, tr, trf};
use mixtape_library::{likes, scrobbles};
use mixtape_player::{PlayerEvent, Status};
use mixtape_sync::lastfm::{LastFm, LastFmError, PlayAction, PlayMeta, PlayTracker, Session};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{Manager, State};
use ts_rs::TS;

/// API account of the app; absent = scrobbling is not available in this build.
const CREDENTIALS: Option<(&str, &str)> = match (
    option_env!("MIXTAPE_LASTFM_KEY"),
    option_env!("MIXTAPE_LASTFM_SECRET"),
) {
    (Some(k), Some(s)) if !k.is_empty() && !s.is_empty() => Some((k, s)),
    _ => None,
};

/// After the browser opens, the session is asked for at these delays (the user may also
/// press «I've allowed access»); then the app stops asking.
const SESSION_TRIES: [u64; 5] = [15, 30, 60, 120, 240];

/// The «Last.fm and Discord» section of settings.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Integrations {
    pub lastfm_available: bool,
    /// The connected Last.fm user.
    #[ts(optional)]
    pub lastfm_user: Option<String>,
    /// Waiting for the user to allow access in the browser.
    pub lastfm_pending: bool,
    pub discord_available: bool,
}

#[derive(Default)]
pub struct Scrobbler {
    api: OnceLock<LastFm>,
    session: Mutex<Option<Session>>,
    tracker: Mutex<PlayTracker>,
    /// Token waiting for the user's approval, with its sign-in attempt.
    pending: Mutex<Option<(String, u64)>>,
    attempt: AtomicU64,
    flushing: AtomicBool,
    retry_armed: AtomicBool,
    /// Flushes in a row that found Last.fm unreachable (backs the retry off).
    unreachable: AtomicU32,
}

impl Scrobbler {
    fn api(&self) -> Result<&LastFm, AppError> {
        let (key, secret) = CREDENTIALS.ok_or_else(|| {
            AppError::new(
                ErrorKind::Invalid,
                tr(
                    "Scrobbling isn't available in this build",
                    "Скроблінг недоступний у цій збірці",
                    "Скробблинг недоступен в этой сборке",
                ),
            )
        })?;
        if let Some(a) = self.api.get() {
            return Ok(a);
        }
        let a = LastFm::new(key, secret)
            .map_err(|e| AppError::new(ErrorKind::Internal, e.to_string()))?;
        Ok(self.api.get_or_init(|| a))
    }

    fn session_key(&self) -> Option<String> {
        self.session
            .lock()
            .expect("lastfm session poisoned")
            .as_ref()
            .map(|s| s.key.clone())
    }
}

fn sealed_path(st: &AppState) -> std::path::PathBuf {
    st.paths.root.join("lastfm.bin")
}

fn now_ms() -> u64 {
    mixtape_library::repo::now_ms().max(0) as u64
}

/// Reads the stored session and sends what waited in the outbox.
pub fn install(st: &Arc<AppState>) {
    let s = Scrobbler::default();
    if CREDENTIALS.is_some()
        && let Ok(sealed) = std::fs::read(sealed_path(st))
    {
        match crate::account::open(&sealed).and_then(|t| {
            serde_json::from_str::<Session>(&t)
                .map_err(|e| AppError::new(ErrorKind::Auth, e.to_string()))
        }) {
            Ok(session) => *s.session.lock().expect("lastfm session poisoned") = Some(session),
            Err(e) => tracing::warn!(error = %e, "stored Last.fm session is unreadable"),
        }
    }
    st.app.manage(s);
    flush(st);
}

fn scrobbler(st: &AppState) -> Option<tauri::State<'_, Scrobbler>> {
    st.app.try_state::<Scrobbler>()
}

fn meta(t: &Track) -> Option<PlayMeta> {
    let artist = t
        .artists
        .first()
        .map(|a| a.name.trim())
        .filter(|a| !a.is_empty())?;
    let title = Some(t.title.trim()).filter(|s| !s.is_empty())?;
    Some(PlayMeta {
        artist: artist.to_owned(),
        track: title.to_owned(),
        album: t.album.as_ref().map(|a| a.title.clone()),
        duration_ms: t.duration_ms,
    })
}

/// Follows the player. Called on the player's event thread; network and disk work goes
/// to the runtime.
pub fn on_player_event(st: &Arc<AppState>, ev: &PlayerEvent) {
    if CREDENTIALS.is_none() {
        return;
    }
    let Some(s) = scrobbler(st) else { return };
    if s.session.lock().expect("lastfm session poisoned").is_none() {
        return;
    }
    let now = now_ms();
    let actions = match ev {
        PlayerEvent::TrackChanged { track, .. } => s
            .tracker
            .lock()
            .expect("tracker poisoned")
            .track_changed(track.id.as_str(), meta(track), now),
        PlayerEvent::State(p) => {
            let mut tracker = s.tracker.lock().expect("tracker poisoned");
            let mut out = Vec::new();
            // Restored at start before this was installed: read the track from the library.
            // Never from the player: this runs inside its event callback, under its lock.
            if !tracker.has_play()
                && let Some(id) = p.track_id.as_ref()
                && let Some(t) = st.db.read(|c| mixtape_library::repo::track(c, id)).ok().flatten()
            {
                out.extend(tracker.track_changed(t.id.as_str(), meta(&t), now));
            }
            let id = p.track_id.as_ref().map(|t| t.as_str());
            out.extend(tracker.state(id, p.status == Status::Playing, p.duration_ms, now));
            out
        }
        _ => return,
    };
    for a in actions {
        run(st, a);
    }
}

fn run(st: &Arc<AppState>, action: PlayAction) {
    let st = st.clone();
    match action {
        PlayAction::NowPlaying(m) => {
            tauri::async_runtime::spawn(async move {
                let Some(s) = scrobbler(&st) else { return };
                let (Ok(api), Some(sk)) = (s.api(), s.session_key()) else {
                    return;
                };
                if let Err(e) = api.update_now_playing(&sk, &m).await {
                    tracing::debug!(error = %e, "Last.fm now playing not sent");
                    if e.is_auth() {
                        session_expired(&st);
                    }
                }
            });
        }
        PlayAction::Scrobble(row) => {
            tauri::async_runtime::spawn_blocking(move || {
                match st.db.write(move |t| scrobbles::queue(t, &row)) {
                    // While a retry waits (offline, Last.fm down) the new row waits with it;
                    // `net.rs` flushes when the network returns.
                    Ok(_) => {
                        let waiting =
                            scrobbler(&st).is_none_or(|s| s.retry_armed.load(Ordering::SeqCst));
                        if !waiting && st.online.load(Ordering::SeqCst) {
                            flush(&st);
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "scrobble not queued"),
                }
            });
        }
    }
}

/// Sends queued scrobbles in the background; one flush at a time.
pub fn flush(st: &Arc<AppState>) {
    let Some(s) = scrobbler(st) else { return };
    if s.session_key().is_none() || s.flushing.swap(true, Ordering::SeqCst) {
        return;
    }
    let st = st.clone();
    tauri::async_runtime::spawn(async move {
        let Some(s) = scrobbler(&st) else { return };
        let mut report = mixtape_sync::lastfm::FlushReport::default();
        if let (Ok(api), Some(sk)) = (s.api(), s.session_key()) {
            let now_s = (now_ms() / 1000) as i64;
            report = mixtape_sync::lastfm::flush(api, &st.db, &sk, now_s).await;
            // Scrobbles queued while that flush ran have not been tried yet.
            if !report.auth_expired
                && report.failed == 0
                && st.db.read(scrobbles::oldest_attempts).ok().flatten() == Some(0)
            {
                report = mixtape_sync::lastfm::flush(api, &st.db, &sk, now_s).await;
            }
        }
        s.flushing.store(false, Ordering::SeqCst);
        let unreachable = if report.unreachable {
            s.unreachable.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            s.unreachable.store(0, Ordering::SeqCst);
            0
        };
        if report.auth_expired {
            session_expired(&st);
        } else {
            arm_retry(&st, unreachable);
        }
        if report.sent + report.failed + report.dropped > 0 {
            tracing::info!(?report, "scrobbles flushed");
        }
    });
}

/// Schedules the next flush while the outbox still has rows, backing off by the oldest
/// row's attempts or by the flushes in a row that found Last.fm unreachable.
fn arm_retry(st: &Arc<AppState>, unreachable: u32) {
    let Some(s) = scrobbler(st) else { return };
    let Ok(Some(attempts)) = st.db.read(scrobbles::oldest_attempts) else {
        return;
    };
    if s.retry_armed.swap(true, Ordering::SeqCst) {
        return;
    }
    let delay = likes::retry_delay(attempts.max(unreachable));
    let st = st.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        if let Some(s) = scrobbler(&st) {
            s.retry_armed.store(false, Ordering::SeqCst);
        }
        // Offline: `net.rs` flushes when the network is back.
        if st.online.load(Ordering::SeqCst) {
            flush(&st);
        }
    });
}

/// Last.fm revoked the session: forget it and tell the user.
fn session_expired(st: &AppState) {
    let Some(s) = scrobbler(st) else { return };
    if s.session
        .lock()
        .expect("lastfm session poisoned")
        .take()
        .is_none()
    {
        return;
    }
    let _ = std::fs::remove_file(sealed_path(st));
    crate::events::toast_error(
        &st.app,
        tr(
            "Last.fm disconnected Mixtape — connect it again in Settings",
            "Last.fm відключив Mixtape — підключіть його знову в Налаштуваннях",
            "Last.fm отключил Mixtape — подключите его снова в Настройках",
        ),
    );
    crate::host::emit(&st.app, "lastfm:changed", ());
}

fn status(st: &AppState) -> Integrations {
    let s = scrobbler(st);
    let user = s.as_ref().and_then(|s| {
        s.session
            .lock()
            .expect("lastfm session poisoned")
            .as_ref()
            .map(|x| x.name.clone())
    });
    let pending = s
        .as_ref()
        .is_some_and(|s| s.pending.lock().expect("lastfm pending poisoned").is_some());
    Integrations {
        lastfm_available: CREDENTIALS.is_some(),
        lastfm_pending: pending && user.is_none(),
        lastfm_user: user,
        discord_available: crate::discord::APP_ID.is_some(),
    }
}

fn api_error(e: LastFmError) -> AppError {
    let kind = if matches!(e, LastFmError::Network(_)) {
        ErrorKind::Network
    } else {
        ErrorKind::Auth
    };
    AppError::new(kind, trf!("Last.fm: {e}", "Last.fm: {e}", "Last.fm: {e}"))
}

/// Stores the session sealed and starts scrobbling.
fn save_session(st: &Arc<AppState>, session: Session) -> Result<(), AppError> {
    let text = serde_json::to_string(&session)
        .map_err(|e| AppError::new(ErrorKind::Internal, e.to_string()))?;
    std::fs::write(sealed_path(st), crate::account::seal(&text)?).map_err(|e| {
        AppError::new(
            ErrorKind::Storage,
            trf!(
                "Couldn't save the sign-in: {e}",
                "Не вдалося зберегти вхід: {e}",
                "Не удалось сохранить вход: {e}"
            ),
        )
    })?;
    if let Some(s) = scrobbler(st) {
        *s.pending.lock().expect("lastfm pending poisoned") = None;
        *s.session.lock().expect("lastfm session poisoned") = Some(session);
        // Start counting from the next track.
        *s.tracker.lock().expect("tracker poisoned") = PlayTracker::default();
    }
    crate::host::emit(&st.app, "lastfm:changed", ());
    Ok(())
}

/// Asks Last.fm whether the user allowed the pending token. `Ok(false)`: not yet.
async fn try_session(st: &Arc<AppState>, attempt: u64) -> Result<bool, AppError> {
    let Some(s) = scrobbler(st) else {
        return Ok(false);
    };
    let token = match s.pending.lock().expect("lastfm pending poisoned").clone() {
        Some((t, a)) if a == attempt => t,
        _ => return Ok(false),
    };
    match s.api()?.get_session(&token).await {
        Ok(session) => {
            // Disconnected or restarted meanwhile.
            if s.attempt.load(Ordering::SeqCst) != attempt {
                return Ok(false);
            }
            save_session(st, session)?;
            Ok(true)
        }
        Err(e) if e.not_authorized_yet() => Ok(false),
        Err(e) if e.token_dead() => {
            *s.pending.lock().expect("lastfm pending poisoned") = None;
            crate::host::emit(&st.app, "lastfm:changed", ());
            Err(AppError::new(
                ErrorKind::Auth,
                tr(
                    "The Last.fm sign-in expired, connect again",
                    "Вхід у Last.fm застарів, підключіться ще раз",
                    "Вход в Last.fm устарел, подключитесь ещё раз",
                ),
            ))
        }
        Err(e) => Err(api_error(e)),
    }
}

#[tauri::command]
pub fn integrations_status(state: State<'_, Arc<AppState>>) -> Integrations {
    status(&state)
}

/// Starts the sign-in: a token, the Last.fm page in the browser, then a few spaced checks.
#[tauri::command]
pub async fn lastfm_connect(state: State<'_, Arc<AppState>>) -> CmdResult<Integrations> {
    let st = state.inner().clone();
    let s = scrobbler(&st).ok_or_else(|| AppError::new(ErrorKind::Internal, "not ready"))?;
    let token = s.api()?.get_token().await.map_err(api_error)?;
    let attempt = s.attempt.fetch_add(1, Ordering::SeqCst) + 1;
    let url = s.api()?.auth_url(&token);
    *s.pending.lock().expect("lastfm pending poisoned") = Some((token, attempt));
    if !mixtape_win::open_url(&url) {
        tracing::warn!("browser did not open for the Last.fm sign-in");
    }
    let st2 = st.clone();
    tauri::async_runtime::spawn(async move {
        for secs in SESSION_TRIES {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            let current =
                scrobbler(&st2).is_some_and(|s| s.attempt.load(Ordering::SeqCst) == attempt);
            if !current {
                return;
            }
            match try_session(&st2, attempt).await {
                Ok(true) | Err(_) => return,
                Ok(false) => {}
            }
        }
    });
    Ok(status(&st))
}

/// «I've allowed access»: checks right away.
#[tauri::command]
pub async fn lastfm_confirm(state: State<'_, Arc<AppState>>) -> CmdResult<Integrations> {
    let st = state.inner().clone();
    let attempt = scrobbler(&st)
        .map(|s| s.attempt.load(Ordering::SeqCst))
        .unwrap_or_default();
    if !try_session(&st, attempt).await? && status(&st).lastfm_user.is_none() {
        return Err(AppError::new(
            ErrorKind::Auth,
            tr(
                "Last.fm hasn't confirmed access yet: press «Yes, allow access» on its page",
                "Last.fm ще не підтвердив доступ: натисніть «Yes, allow access» на його сторінці",
                "Last.fm ещё не подтвердил доступ: нажмите «Yes, allow access» на его странице",
            ),
        ));
    }
    Ok(status(&st))
}

/// Forgets the session and the scrobbles that waited for it.
#[tauri::command]
pub async fn lastfm_disconnect(state: State<'_, Arc<AppState>>) -> CmdResult<Integrations> {
    let st = state.inner().clone();
    blocking(move || {
        if let Some(s) = scrobbler(&st) {
            s.attempt.fetch_add(1, Ordering::SeqCst);
            *s.pending.lock().expect("lastfm pending poisoned") = None;
            *s.session.lock().expect("lastfm session poisoned") = None;
            *s.tracker.lock().expect("tracker poisoned") = PlayTracker::default();
        }
        let _ = std::fs::remove_file(sealed_path(&st));
        st.db.write(|t| scrobbles::clear(t))?;
        Ok(status(&st))
    })
    .await
}
