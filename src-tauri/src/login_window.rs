//! Sign-in in a Mixtape window (T064): Google's own login page in a private WebView2
//! window; once music.youtube.com is open with a session, its cookies are taken from
//! the webview and connected like an imported cookies file. The window is incognito,
//! so the session is used only by Mixtape and Google does not rotate it under us.

use crate::account::{self, AccountStatus};
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind};
use std::fmt::Write as _;
use std::time::{Duration, Instant};
use tauri::webview::cookie::Cookie;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const LABEL: &str = "login";
const START: &str = "https://accounts.google.com/ServiceLogin?service=youtube&passive=true&hl=ru\
    &continue=https%3A%2F%2Fmusic.youtube.com%2F";
const POLL: Duration = Duration::from_millis(800);
const GIVE_UP: Duration = Duration::from_secs(15 * 60);

fn cancelled() -> AppError {
    AppError::new(ErrorKind::Auth, "Вход отменён")
}

pub async fn sign_in(st: &AppState) -> Result<AccountStatus, AppError> {
    if let Some(w) = st.app.get_webview_window(LABEL) {
        let _ = w.set_focus();
        return Err(AppError::new(ErrorKind::Invalid, "Окно входа уже открыто"));
    }
    let url = START.parse().map_err(|e| AppError::new(ErrorKind::Internal, format!("{e}")))?;
    let window = WebviewWindowBuilder::new(&st.app, LABEL, WebviewUrl::External(url))
        .title("Вход в YouTube Music")
        .inner_size(480.0, 720.0)
        .center()
        .incognito(true)
        .build()
        .map_err(|e| AppError::new(ErrorKind::Internal, format!("Не удалось открыть окно входа: {e}")))?;

    let music = "https://music.youtube.com/".parse().expect("static url");
    let started = Instant::now();
    loop {
        tokio::time::sleep(POLL).await;
        if st.app.get_webview_window(LABEL).is_none() {
            return Err(cancelled());
        }
        if started.elapsed() > GIVE_UP {
            let _ = window.close();
            return Err(cancelled());
        }
        let on_music = window.url().is_ok_and(|u| u.host_str() == Some("music.youtube.com"));
        if !on_music {
            continue;
        }
        // Reading cookies blocks until WebView2 answers; this runs on a runtime thread,
        // never on the UI thread (see Tauri's note on the WebView2 deadlock).
        let Ok(cookies) = window.cookies_for_url(Url::clone(&music)) else { continue };
        if !cookies.iter().any(|c| c.name() == "SAPISID" || c.name() == "__Secure-3PAPISID") {
            continue;
        }
        let result = account::connect(st, &netscape(&cookies)).await;
        let _ = window.close();
        return result;
    }
}

type Url = tauri::Url;

fn netscape(cookies: &[Cookie<'static>]) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for c in cookies {
        let domain = c.domain().map(|d| if d.starts_with('.') { d.to_owned() } else { format!(".{d}") });
        let domain = domain.unwrap_or_else(|| ".youtube.com".into());
        let expires = c.expires_datetime().map(|t| t.unix_timestamp().max(0)).unwrap_or(0);
        let secure = if c.secure().unwrap_or(false) { "TRUE" } else { "FALSE" };
        let _ = writeln!(out, "{domain}\tTRUE\t{}\t{secure}\t{expires}\t{}\t{}", c.path().unwrap_or("/"), c.name(), c.value());
    }
    out
}
