//! Sign-in in a Mixtape window (T064): Google's own login page in a private WebView2
//! window; once music.youtube.com is open with a session, its cookies are taken from
//! the webview and connected like an imported cookies file. The window is incognito,
//! so the session is used only by Mixtape and Google does not rotate it under us.

use crate::account::{self, AccountStatus};
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind, tr, trf};
use std::fmt::Write as _;
use std::time::{Duration, Instant};
use tauri::webview::cookie::Cookie;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const LABEL: &str = "login";
/// Google's sign-in page, in the app language.
fn start_url() -> String {
    format!(
        "https://accounts.google.com/ServiceLogin?service=youtube&passive=true&hl={}\
         &continue=https%3A%2F%2Fmusic.youtube.com%2F",
        mixtape_core::l10n::lang().code()
    )
}
const POLL: Duration = Duration::from_millis(800);
const GIVE_UP: Duration = Duration::from_secs(15 * 60);

fn cancelled() -> AppError {
    AppError::new(ErrorKind::Auth, tr("Sign-in cancelled", "Вхід скасовано", "Вход отменён"))
}

pub async fn sign_in(st: &AppState) -> Result<AccountStatus, AppError> {
    // A second click just brings the open window forward and waits on it as well.
    let window = match st.app.get_webview_window(LABEL) {
        Some(w) => w,
        None => {
            let url = start_url().parse().map_err(|e| AppError::new(ErrorKind::Internal, format!("{e}")))?;
            let mut builder = WebviewWindowBuilder::new(&st.app, LABEL, WebviewUrl::External(url))
                .title(tr("Sign in to YouTube Music", "Вхід у YouTube Music", "Вход в YouTube Music"))
                .inner_size(480.0, 720.0)
                .center()
                .focused(true)
                .incognito(true)
                // Only Google's sign-in and YouTube; links elsewhere and pop-ups go nowhere.
                .on_navigation(|url| {
                    let ok = allowed_navigation(url);
                    if !ok {
                        tracing::info!(host = ?url.host_str(), "sign-in window: navigation blocked");
                    }
                    ok
                })
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
            // Owned by the main window, so it always stays above it.
            if let Some(main) = st.app.get_webview_window("main") {
                builder = builder.parent(&main).map_err(|e| AppError::new(ErrorKind::Internal, e.to_string()))?;
            }
            builder
                .build()
                .map_err(|e| {
                    AppError::new(
                        ErrorKind::Internal,
                        trf!(
                            "Couldn't open the sign-in window: {e}",
                            "Не вдалося відкрити вікно входу: {e}",
                            "Не удалось открыть окно входа: {e}",
                        ),
                    )
                })?
        }
    };
    let _ = window.unminimize();
    let _ = window.set_focus();

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
        if result.is_ok() {
            // The page that asked may have reloaded meanwhile; tell every listener.
            let _ = st.app.emit("account:changed", ());
        }
        return result;
    }
}

type Url = tauri::Url;

/// Pages the sign-in window may open: https on Google's sign-in hosts (`accounts.google.com`
/// and the rest of `google.com`, plus the country domains Google hops through to set its
/// session, e.g. `accounts.google.co.uk`) and on YouTube (`accounts.youtube.com`,
/// `consent.youtube.com`, `music.youtube.com`), and `about:blank`.
fn allowed_navigation(url: &Url) -> bool {
    if url.as_str() == "about:blank" {
        return true;
    }
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else { return false };
    let on = |domain: &str| {
        host == domain || host.strip_suffix(domain).is_some_and(|sub| sub.len() > 1 && sub.ends_with('.'))
    };
    if on("google.com") || on("youtube.com") {
        return true;
    }
    // google.<cc>, google.co.<cc>, google.com.<cc> and their subdomains.
    let labels: Vec<&str> = host.split('.').collect();
    let cc = |l: &str| l.len() == 2 && l.bytes().all(|b| b.is_ascii_lowercase());
    match labels.as_slice() {
        [.., "google", tld] => cc(tld),
        [.., "google", "co" | "com", tld] => cc(tld),
        _ => false,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_window_stays_on_google_and_youtube() {
        for ok in [
            "https://accounts.google.com/ServiceLogin?service=youtube",
            "https://accounts.google.com/v3/signin/challenge/pwd",
            "https://myaccount.google.com/",
            "https://accounts.youtube.com/accounts/SetSID",
            "https://consent.youtube.com/m",
            "https://music.youtube.com/",
            "https://www.youtube.com/signin",
            "https://accounts.google.co.uk/accounts/SetSID",
            "https://www.google.com.ua/",
            "https://google.de/",
            "about:blank",
        ] {
            assert!(allowed_navigation(&ok.parse().unwrap()), "{ok}");
        }
        for bad in [
            "http://accounts.google.com/",
            "https://evil.com/",
            "https://google.com.evil.com/",
            "https://accounts.google.com.evil.io/",
            "https://evilgoogle.com/",
            "https://notyoutube.com/",
            "https://user@accounts.google.com/",
            "https://google.example/",
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/html,hi",
        ] {
            assert!(!allowed_navigation(&bad.parse().unwrap()), "{bad}");
        }
    }
}
