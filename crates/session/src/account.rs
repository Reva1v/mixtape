//! The connected YouTube Music account: cookies sealed with DPAPI in `account.bin`, the
//! name and avatar for the UI in settings, and the hand-off of cookies to the InnerTube
//! client and yt-dlp.

use crate::state::Session;
use mixtape_core::{AppError, ErrorKind, tr, trf};
use mixtape_library::settings;
use mixtape_yt::api::YtMusic;
use mixtape_yt::auth::Cookies;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use ts_rs::TS;

const KEY: &str = "account";
pub const AVATAR_KEY: &str = "account-avatar";

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountStatus {
    pub connected: bool,
    #[ts(optional)]
    pub name: Option<String>,
    /// Cover key of the avatar.
    #[ts(optional)]
    pub avatar: Option<String>,
    /// Unix ms of the last successful sync.
    #[ts(optional, type = "number")]
    pub last_sync_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Stored {
    name: String,
    avatar: Option<String>,
}

fn sealed_path(st: &Session) -> PathBuf {
    st.paths.root.join("account.bin")
}

/// What this process knows about the session besides the jar itself.
#[derive(Default)]
pub struct AccountHealth {
    /// YouTube confirmed the jar is signed out; it stays so until a new jar.
    dead: AtomicBool,
    /// A confirmation request is on its way.
    checking: AtomicBool,
    /// This jar was asked about once: until then «connected» only means it is on disk.
    looked: AtomicBool,
    /// `account.bin` as this process last read or wrote it, to notice another process's write.
    sealed: Mutex<Option<Vec<u8>>>,
}

impl AccountHealth {
    fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    /// True if a confirmation should start: not one at a time, and none for a dead jar.
    fn begin_check(&self) -> bool {
        !self.is_dead() && !self.checking.swap(true, Ordering::SeqCst)
    }

    /// Ends the confirmation; true if this is the moment to tell the user the session is gone.
    fn finish_check(&self, signed_out: bool) -> bool {
        self.checking.store(false, Ordering::SeqCst);
        signed_out && !self.dead.swap(true, Ordering::SeqCst)
    }

    fn revive(&self) {
        self.dead.store(false, Ordering::SeqCst);
        self.looked.store(false, Ordering::SeqCst);
    }

    /// True the first time a jar is shown as connected.
    fn first_look(&self) -> bool {
        !self.looked.swap(true, Ordering::SeqCst)
    }

    fn sealed(&self) -> std::sync::MutexGuard<'_, Option<Vec<u8>>> {
        self.sealed.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Reads the stored cookies at startup and hands them to the clients.
pub fn restore(st: &Session) {
    let Ok(sealed) = std::fs::read(sealed_path(st)) else {
        return;
    };
    match open(&sealed).and_then(|text| Cookies::parse(&text).map_err(AppError::from)) {
        Ok(cookies) => {
            *st.account.sealed() = Some(sealed);
            apply(st, Some(cookies));
        }
        Err(e) => tracing::warn!(error = %e, "stored YouTube cookies are unreadable"),
    }
}

fn apply(st: &Session, cookies: Option<Cookies>) {
    st.resolver
        .set_cookies(cookies.as_ref().map(Cookies::to_netscape));
    st.yt.set_cookies(cookies);
    st.account.revive();
    st.catalog_pages.clear();
}

/// There is a jar and YouTube has not refused it: worth sending requests with.
pub fn usable(st: &Session) -> bool {
    st.yt.is_authenticated() && !st.account.is_dead()
}

/// Seals the jar and replaces `account.bin` in one step, so the other process sharing the
/// file never reads half of it.
fn save(st: &Session, cookies: &Cookies) -> Result<(), AppError> {
    let sealed = seal(&cookies.to_netscape())?;
    let path = sealed_path(st);
    let tmp = path.with_extension("bin.tmp");
    std::fs::write(&tmp, &sealed)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| {
            AppError::new(
                ErrorKind::Storage,
                trf!(
                    "Couldn't save the sign-in: {e}",
                    "Не вдалося зберегти вхід: {e}",
                    "Не удалось сохранить вход: {e}"
                ),
            )
        })?;
    *st.account.sealed() = Some(sealed);
    Ok(())
}

/// Stores what YouTube changed in the jar and passes it on to yt-dlp.
fn persist_changes(st: &Session) {
    // A signed out jar is not worth a write: it would only wake the other process.
    if !st.yt.take_changed() || st.account.is_dead() {
        return;
    }
    let Some(cookies) = st.yt.cookies() else {
        return;
    };
    st.resolver.set_cookies(Some(cookies.to_netscape()));
    if let Err(e) = save(st, &cookies) {
        tracing::warn!(error = %e.message, "refreshed YouTube cookies not saved");
    }
}

/// Picks up `account.bin` if another process changed it: the app and the island share one
/// account, each holding the jar in memory.
pub fn reload_from_disk(st: &Session) {
    let path = sealed_path(st);
    match std::fs::read(&path) {
        Ok(bytes) => {
            if st.account.sealed().as_deref() == Some(bytes.as_slice()) {
                return;
            }
            match open(&bytes).and_then(|text| Cookies::parse(&text).map_err(AppError::from)) {
                Ok(cookies) => {
                    *st.account.sealed() = Some(bytes);
                    // The same session refreshed by the other process: what is known about it
                    // holds, and a signed out one stays signed out.
                    if st.yt.cookies().is_some_and(|c| c.same_session(&cookies)) {
                        st.resolver.set_cookies(Some(cookies.to_netscape()));
                        st.yt.set_cookies(Some(cookies));
                        return;
                    }
                    apply(st, Some(cookies));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "stored YouTube cookies are unreadable");
                    return;
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if st.account.sealed().take().is_none() {
                return;
            }
            apply(st, None);
        }
        // The other process is replacing it right now; next time.
        Err(_) => return,
    }
    tracing::info!("YouTube account changed by another process");
    crate::host::emit(&st.app, "account:changed", ());
}

/// One round of what an open YouTube tab does for its session: refresh the short-lived cookies
/// and keep the result. Only the process that owns the session may call this: two rotating one
/// jar on their own would invalidate each other. False if it should be tried again soon.
pub async fn keep_alive(st: &Session) -> bool {
    reload_from_disk(st);
    if !usable(st) {
        return true;
    }
    let rotated = st.yt.rotate().await;
    persist_changes(st);
    match rotated {
        Ok(()) => true,
        Err(mixtape_yt::YtError::Auth) => {
            auth_failed(&st.app, false);
            true
        }
        Err(e) => {
            tracing::debug!(error = %e, "YouTube session not refreshed");
            false
        }
    }
}

/// A request was refused as signed out. A refusal alone proves little (a 403, yt-dlp's «sign in
/// to confirm you're not a bot»), so one signed-in request decides; a session really gone is
/// announced once per jar. `downloads_paused`: the caller stopped the downloads over this.
pub fn auth_failed(app: &AppHandle, downloads_paused: bool) {
    let Some(st) = app.try_state::<Arc<Session>>() else {
        return;
    };
    let st = st.inner().clone();
    if !st.account.begin_check() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let generation = st.yt.generation();
        let verdict = st.yt.browse_signed_in("VLLM").await;
        let signed_out =
            matches!(verdict, Err(mixtape_yt::YtError::Auth)) && st.yt.generation() == generation;
        if st.account.finish_check(signed_out) {
            tracing::warn!("YouTube session is signed out");
            crate::host::emit(&st.app, "account:expired", ());
        } else if !signed_out && verdict.is_ok() {
            tracing::warn!("a YouTube request was refused, yet the session is signed in");
            if downloads_paused {
                crate::events::toast_error(
                    &st.app,
                    tr(
                        "YouTube refused the download, downloads are paused. Try again later",
                        "YouTube відмовив у завантаженні, завантаження на паузі. Спробуйте пізніше",
                        "YouTube отказал в загрузке, загрузки на паузе. Попробуйте позже",
                    ),
                );
            }
        }
    });
}

#[cfg(windows)]
pub(crate) fn seal(text: &str) -> Result<Vec<u8>, AppError> {
    mixtape_win::dpapi::protect(text.as_bytes()).map_err(|e| {
        tracing::warn!(error = %e, "DPAPI protect failed");
        AppError::new(
            ErrorKind::Internal,
            mixtape_core::tr(
                "Couldn't encrypt the account data on this computer",
                "Не вдалося зашифрувати дані акаунта на цьому комп'ютері",
                "Не удалось зашифровать данные аккаунта на этом компьютере",
            ),
        )
    })
}

#[cfg(windows)]
pub(crate) fn open(sealed: &[u8]) -> Result<String, AppError> {
    let unreadable = || {
        AppError::new(
            ErrorKind::Auth,
            mixtape_core::tr(
                "The saved sign-in can't be read on this computer, connect the account again",
                "Збережений вхід не читається на цьому комп'ютері — підключіть акаунт знову",
                "Сохранённый вход не читается на этом компьютере — подключите аккаунт заново",
            ),
        )
    };
    let bytes = mixtape_win::dpapi::unprotect(sealed).map_err(|e| {
        tracing::warn!(error = %e, "DPAPI unprotect failed");
        unreadable()
    })?;
    String::from_utf8(bytes).map_err(|_| unreadable())
}

#[cfg(not(windows))]
pub(crate) fn seal(text: &str) -> Result<Vec<u8>, AppError> {
    Ok(text.as_bytes().to_vec())
}

#[cfg(not(windows))]
pub(crate) fn open(sealed: &[u8]) -> Result<String, AppError> {
    String::from_utf8(sealed.to_vec()).map_err(|e| AppError::new(ErrorKind::Auth, e.to_string()))
}

pub fn status(st: &Session) -> AccountStatus {
    if !usable(st) {
        return AccountStatus::default();
    }
    // A process that is not the session's owner sends nothing by itself, so without this it would
    // show the profile of a session long signed out.
    if st.account.first_look() {
        auth_failed(&st.app, false);
    }
    let stored: Stored = settings::get(&st.db, KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    AccountStatus {
        connected: true,
        name: Some(stored.name).filter(|n| !n.is_empty()),
        avatar: stored.avatar,
        last_sync_at: st
            .db
            .read(mixtape_library::ytm::last_synced_at)
            .ok()
            .flatten(),
    }
}

/// Checks the cookies against YouTube, then stores them sealed.
pub async fn connect(st: &Session, text: &str) -> Result<AccountStatus, AppError> {
    let cookies = Cookies::parse(text)?;
    let previous = st.yt.cookies();
    st.yt.set_cookies(Some(cookies.clone()));
    let account = match st.yt.account().await {
        Ok(a) => a,
        Err(e) => {
            st.yt.set_cookies(previous.map(|c| (*c).clone()));
            return Err(match e {
                mixtape_yt::YtError::Auth => AppError::new(
                    ErrorKind::Auth,
                    tr(
                        "YouTube didn't accept these cookies, sign in again and export them once more",
                        "YouTube не прийняв ці cookies — увійдіть знову й експортуйте їх ще раз",
                        "YouTube не принял эти cookies — войдите заново и экспортируйте их ещё раз",
                    ),
                ),
                e => e.into(),
            });
        }
    };
    // The check above may already have refreshed the jar.
    let cookies = st.yt.cookies().map_or(cookies, |c| (*c).clone());
    save(st, &cookies)?;
    apply(st, Some(cookies));
    // A fresh jar starts its 10-minute cycle at once.
    if let Err(e) = st.yt.rotate().await {
        tracing::debug!(error = %e, "new YouTube session not refreshed");
    }
    persist_changes(st);

    let mut avatar = None;
    if let Some(url) = &account.avatar
        && let Ok(bytes) = mixtape_yt::thumbs::download_small(st.yt.http(), url).await
        && st
            .covers
            .process(AVATAR_KEY, &bytes, mixtape_artwork::Detail::Small)
            .is_ok()
    {
        avatar = Some(AVATAR_KEY.to_owned());
    }
    settings::set(
        &st.db,
        KEY,
        &Stored {
            name: account.name,
            avatar,
        },
    )?;
    Ok(status(st))
}

pub fn disconnect(st: &Session) -> Result<(), AppError> {
    st.downloads.pause();
    apply(st, None);
    let _ = std::fs::remove_file(sealed_path(st));
    *st.account.sealed() = None;
    settings::set(&st.db, KEY, &Stored::default())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dead_session_is_announced_once() {
        let h = AccountHealth::default();
        assert!(h.begin_check());
        assert!(h.finish_check(true), "first confirmation tells the user");
        assert!(h.is_dead());
        assert!(
            !h.begin_check(),
            "no more requests or notices for a dead jar"
        );
        assert!(!h.finish_check(true), "a late confirmation stays silent");
    }

    #[test]
    fn a_jar_is_looked_at_once_until_a_new_one() {
        let h = AccountHealth::default();
        assert!(h.first_look());
        assert!(!h.first_look());
        h.revive();
        assert!(h.first_look());
    }

    #[test]
    fn only_one_confirmation_runs() {
        let h = AccountHealth::default();
        assert!(h.begin_check());
        assert!(!h.begin_check());
        assert!(!h.finish_check(false), "a live session announces nothing");
        assert!(!h.is_dead());
        assert!(h.begin_check(), "the next refusal may check again");
    }

    #[test]
    fn a_new_jar_can_be_announced_again() {
        let h = AccountHealth::default();
        assert!(h.begin_check());
        assert!(h.finish_check(true));
        h.revive();
        assert!(h.begin_check());
        assert!(h.finish_check(true));
    }
}
