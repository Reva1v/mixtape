//! The connected YouTube Music account: cookies sealed with DPAPI in `account.bin`, the
//! name and avatar for the UI in settings, and the hand-off of cookies to the InnerTube
//! client and yt-dlp.

use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind};
use mixtape_library::settings;
use mixtape_yt::api::YtMusic;
use mixtape_yt::auth::Cookies;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use ts_rs::TS;

const KEY: &str = "account";
const AVATAR_KEY: &str = "account-avatar";

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

fn sealed_path(st: &AppState) -> PathBuf {
    st.paths.root.join("account.bin")
}

/// Reads the stored cookies at startup and hands them to the clients.
pub fn restore(st: &AppState) {
    let Ok(sealed) = std::fs::read(sealed_path(st)) else { return };
    match open(&sealed).and_then(|text| Cookies::parse(&text).map_err(AppError::from)) {
        Ok(cookies) => apply(st, Some(cookies)),
        Err(e) => tracing::warn!(error = %e, "stored YouTube cookies are unreadable"),
    }
}

fn apply(st: &AppState, cookies: Option<Cookies>) {
    st.resolver.set_cookies(cookies.as_ref().map(Cookies::to_netscape));
    st.yt.set_cookies(cookies);
}

#[cfg(windows)]
fn seal(text: &str) -> Result<Vec<u8>, AppError> {
    mixtape_win::dpapi::protect(text.as_bytes()).map_err(|e| AppError::new(ErrorKind::Internal, e.to_string()))
}

#[cfg(windows)]
fn open(sealed: &[u8]) -> Result<String, AppError> {
    let bytes = mixtape_win::dpapi::unprotect(sealed).map_err(|e| AppError::new(ErrorKind::Auth, e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| AppError::new(ErrorKind::Auth, e.to_string()))
}

#[cfg(not(windows))]
fn seal(text: &str) -> Result<Vec<u8>, AppError> {
    Ok(text.as_bytes().to_vec())
}

#[cfg(not(windows))]
fn open(sealed: &[u8]) -> Result<String, AppError> {
    String::from_utf8(sealed.to_vec()).map_err(|e| AppError::new(ErrorKind::Auth, e.to_string()))
}

pub fn status(st: &AppState) -> AccountStatus {
    if !st.yt.is_authenticated() {
        return AccountStatus::default();
    }
    let stored: Stored = settings::get(&st.db, KEY).ok().flatten().unwrap_or_default();
    AccountStatus {
        connected: true,
        name: Some(stored.name).filter(|n| !n.is_empty()),
        avatar: stored.avatar,
        last_sync_at: st.db.read(mixtape_library::ytm::last_synced_at).ok().flatten(),
    }
}

/// Checks the cookies against YouTube, then stores them sealed.
pub async fn connect(st: &AppState, text: &str) -> Result<AccountStatus, AppError> {
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
                    "YouTube не принял эти cookies — войдите заново и экспортируйте их ещё раз",
                ),
                e => e.into(),
            });
        }
    };
    std::fs::write(sealed_path(st), seal(&cookies.to_netscape())?)
        .map_err(|e| AppError::new(ErrorKind::Storage, format!("Не удалось сохранить вход: {e}")))?;
    apply(st, Some(cookies));

    let mut avatar = None;
    if let Some(url) = &account.avatar
        && let Ok(bytes) = mixtape_yt::thumbs::download(st.yt.http(), url).await
        && st.covers.process(AVATAR_KEY, &bytes).is_ok()
    {
        avatar = Some(AVATAR_KEY.to_owned());
    }
    settings::set(&st.db, KEY, &Stored { name: account.name, avatar })?;
    Ok(status(st))
}

pub fn disconnect(st: &AppState) -> Result<(), AppError> {
    st.downloads.pause();
    apply(st, None);
    let _ = std::fs::remove_file(sealed_path(st));
    settings::set(&st.db, KEY, &Stored::default())?;
    Ok(())
}
