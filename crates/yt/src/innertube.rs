//! Authenticated InnerTube client for music.youtube.com (`WEB_REMIX`): the same requests
//! the web player makes, signed with SAPISIDHASH from the user's cookies.

use crate::auth::{Cookies, ORIGIN};
use crate::parse::find_all;
use crate::{Result, YtError};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API_HOST: &str = "music.youtube.com";
const ROTATE_HOST: &str = "accounts.youtube.com";
const ROTATE_ORIGIN: &str = "https://accounts.youtube.com";
const ROTATE_URL: &str = "https://accounts.youtube.com/RotateCookies";
/// The request body the web client sends; its values are placeholders.
const ROTATE_BODY: &str = r#"[000,"-0000000000000000000"]"#;

const CLIENT_NAME: &str = "WEB_REMIX";
const CLIENT_VERSION: &str = "1.20261013.01.00";
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// Upper bound on continuation pages for one list (100 rows each → 20k rows).
pub const MAX_PAGES: usize = 200;

/// What a request said about the network, for the app's online/offline status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// YouTube answered (with any status).
    Answered,
    /// The connection could not be made: no network, DNS failure, refused.
    Unreachable,
    /// The request timed out: the network may be gone or just slow.
    TimedOut,
}

type ReachHook = Box<dyn Fn(Reach) + Send + Sync>;

pub struct InnerTube {
    http: reqwest::Client,
    cookies: RwLock<Option<Arc<Cookies>>>,
    /// Language YouTube answers in (shelf titles, chips, subtitles); follows the app's.
    hl: RwLock<String>,
    gl: String,
    on_reach: OnceLock<ReachHook>,
    /// Counts the jars given to [`Self::set_cookies`]: an answer to an older one is dropped.
    generation: AtomicU64,
    /// YouTube changed the jar since the last [`Self::take_changed`].
    changed: AtomicBool,
}

impl InnerTube {
    pub fn new(hl: &str, gl: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .gzip(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            cookies: RwLock::new(None),
            hl: RwLock::new(hl.to_owned()),
            gl: gl.to_owned(),
            on_reach: OnceLock::new(),
            generation: AtomicU64::new(0),
            changed: AtomicBool::new(false),
        })
    }

    /// Switches the language of later answers (`en`, `uk`, `ru`).
    pub fn set_language(&self, hl: &str) {
        *self.hl.write().unwrap_or_else(|e| e.into_inner()) = hl.to_owned();
    }

    /// Called after every InnerTube request with what it said about the network; set once.
    pub fn set_reach_hook(&self, hook: impl Fn(Reach) + Send + Sync + 'static) {
        let _ = self.on_reach.set(Box::new(hook));
    }

    fn report(&self, reach: Reach) {
        if let Some(hook) = self.on_reach.get() {
            hook(reach);
        }
    }

    /// Shared HTTP client, also used for covers.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn set_cookies(&self, cookies: Option<Cookies>) {
        let mut slot = self.cookies.write().unwrap_or_else(|e| e.into_inner());
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.changed.store(false, Ordering::SeqCst);
        *slot = cookies.map(Arc::new);
    }

    pub fn cookies(&self) -> Option<Arc<Cookies>> {
        self.cookies
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Changes whenever [`Self::set_cookies`] replaces the jar.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// True once after YouTube changed the jar: time to store it.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }

    /// Takes the rotating cookies from a saved copy of this jar (see [`Cookies::merge_rotating`]).
    pub fn merge_rotating(&self, from: &Cookies) {
        self.update_jar(|jar| jar.merge_rotating(from));
    }

    /// Asks Google to refresh the session's short-lived cookies, as an open YouTube tab does
    /// every 10 minutes. Without this a stored session stops being signed in.
    pub async fn rotate(&self) -> Result<()> {
        let generation = self.generation();
        let Some(cookies) = self.cookies() else {
            return Ok(());
        };
        let resp = self
            .http
            .post(ROTATE_URL)
            .header("Origin", ROTATE_ORIGIN)
            .header("Content-Type", "application/json")
            .header("Cookie", cookies.header())
            .body(ROTATE_BODY)
            .send()
            .await?;
        let status = resp.status();
        match status.as_u16() {
            200..=299 => {}
            401 | 403 => return Err(YtError::Auth),
            429 => return Err(YtError::RateLimited),
            _ => return Err(YtError::Network(format!("RotateCookies: {status}"))),
        }
        self.absorb(generation, &set_cookie_headers(&resp), ROTATE_HOST);
        Ok(())
    }

    /// Stores what an answer set, unless the jar was replaced after the request went out.
    fn absorb(&self, generation: u64, headers: &[String], default_domain: &str) {
        if headers.is_empty() {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let values = headers.iter().map(String::as_str);
        self.update_jar_if(generation, |jar| {
            jar.merge_set_cookies(values, default_domain, now)
        });
    }

    /// As [`Self::absorb`] for an InnerTube answer: one served to a guest carries a session
    /// being cleared, and is not taken.
    fn absorb_answer(&self, generation: u64, headers: &[String], answer: &Value) {
        if logged_in(answer) != Some(false) {
            self.absorb(generation, headers, API_HOST);
        }
    }

    fn update_jar(&self, change: impl FnOnce(&mut Cookies) -> bool) {
        self.update_jar_if(self.generation(), change);
    }

    fn update_jar_if(&self, generation: u64, change: impl FnOnce(&mut Cookies) -> bool) {
        let mut slot = self.cookies.write().unwrap_or_else(|e| e.into_inner());
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let Some(current) = slot.as_ref() else { return };
        let mut next = (**current).clone();
        if change(&mut next) {
            *slot = Some(Arc::new(next));
            self.changed.store(true, Ordering::SeqCst);
        }
    }

    pub fn is_authenticated(&self) -> bool {
        self.cookies().is_some()
    }

    /// POST `/youtubei/v1/{endpoint}` with the client context merged into `body`.
    pub async fn call(&self, endpoint: &str, mut body: Value) -> Result<Value> {
        let hl = self.hl.read().unwrap_or_else(|e| e.into_inner()).clone();
        body["context"] = json!({
            "client": { "clientName": CLIENT_NAME, "clientVersion": CLIENT_VERSION, "hl": hl, "gl": self.gl },
            "user": {}
        });
        let generation = self.generation();
        let mut req = self
            .http
            .post(format!("{ORIGIN}/youtubei/v1/{endpoint}?prettyPrint=false"))
            .header("Origin", ORIGIN)
            .header("X-Origin", ORIGIN)
            .header("Referer", format!("{ORIGIN}/"))
            .header("X-Goog-AuthUser", "0")
            .header("X-Youtube-Client-Name", "67")
            .header("X-Youtube-Client-Version", CLIENT_VERSION)
            .json(&body);
        if let Some(cookies) = self.cookies() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            req = req.header("Cookie", cookies.header());
            if let Some(auth) = cookies.authorization(now) {
                req = req.header("Authorization", auth);
            }
        }
        let resp = match req.send().await {
            Ok(r) => {
                self.report(Reach::Answered);
                r
            }
            Err(e) => {
                if e.is_connect() {
                    self.report(Reach::Unreachable);
                } else if e.is_timeout() {
                    self.report(Reach::TimedOut);
                }
                return Err(e.into());
            }
        };
        let status = resp.status();
        match status.as_u16() {
            200..=299 => {}
            401 | 403 => return Err(YtError::Auth),
            429 => return Err(YtError::RateLimited),
            _ => {
                let msg = read_limited(resp, MAX_ERROR_BYTES)
                    .await
                    .unwrap_or_default();
                let msg = serde_json::from_slice::<Value>(&msg)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/error/message")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| status.to_string());
                return Err(YtError::Network(format!("{endpoint}: {msg}")));
            }
        }
        let set_cookies = set_cookie_headers(&resp);
        let body = read_limited(resp, MAX_JSON_BYTES)
            .await
            .map_err(|e| match e {
                BodyError::Http(e) => YtError::from(e),
                e @ BodyError::TooLarge(_) => YtError::Parse(format!("{endpoint}: {e}")),
            })?;
        let answer: Value = serde_json::from_slice(&body)
            .map_err(|e| YtError::Parse(format!("{endpoint}: {e}")))?;
        self.absorb_answer(generation, &set_cookies, &answer);
        Ok(answer)
    }

    pub async fn browse(&self, browse_id: &str) -> Result<Value> {
        self.call("browse", json!({ "browseId": browse_id })).await
    }

    pub async fn browse_params(&self, browse_id: &str, params: &str) -> Result<Value> {
        self.call("browse", json!({ "browseId": browse_id, "params": params }))
            .await
    }

    pub async fn browse_continuation(&self, token: &str) -> Result<Value> {
        self.call("browse", json!({ "continuation": token })).await
    }

    /// Fails with `Auth` if YouTube served the page as to a guest: cookies are dead.
    pub async fn browse_signed_in(&self, browse_id: &str) -> Result<Value> {
        let page = self.browse(browse_id).await?;
        if logged_in(&page) == Some(false) {
            return Err(YtError::Auth);
        }
        Ok(page)
    }
}

/// Largest JSON answer read into memory: a long library page is a few megabytes.
pub const MAX_JSON_BYTES: usize = 20 * 1024 * 1024;
/// Largest image read into memory.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// Largest error page read for its message.
const MAX_ERROR_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("response is larger than {0} bytes")]
    TooLarge(usize),
}

/// The response body, refused as soon as it grows past `max` bytes (or announces more), so
/// a broken or hostile server cannot make the app hold an unbounded answer in memory.
pub async fn read_limited(
    mut resp: reqwest::Response,
    max: usize,
) -> std::result::Result<Vec<u8>, BodyError> {
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return Err(BodyError::TooLarge(max));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if body.len() + chunk.len() > max {
            return Err(BodyError::TooLarge(max));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn set_cookie_headers(resp: &reqwest::Response) -> Vec<String> {
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect()
}

/// `responseContext.serviceTrackingParams[*].params[logged_in]`.
pub fn logged_in(resp: &Value) -> Option<bool> {
    find_all(resp, "serviceTrackingParams")
        .into_iter()
        .find_map(|services| {
            services.as_array()?.iter().find_map(|s| {
                s.get("params")?.as_array()?.iter().find_map(|p| {
                    (p.get("key")?.as_str()? == "logged_in")
                        .then(|| p.get("value").and_then(Value::as_str) == Some("1"))
                })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAR: &str = ".youtube.com\tTRUE\t/\tFALSE\t0\tSAPISID\tkey\n\
        .youtube.com\tTRUE\t/\tTRUE\t0\tSIDCC\told\n";

    fn client() -> InnerTube {
        let yt = InnerTube::new("ru", "RU").unwrap();
        yt.set_cookies(Some(Cookies::parse(JAR).unwrap()));
        yt
    }

    fn page(flag: &str) -> Value {
        json!({"responseContext": {"serviceTrackingParams": [
            {"service": "GFEEDBACK", "params": [{"key": "logged_in", "value": flag}]}]}})
    }

    fn sidcc(yt: &InnerTube) -> bool {
        yt.cookies().unwrap().header().contains("SIDCC=new")
    }

    const ROTATED: &str = "SIDCC=new; Domain=.youtube.com";

    #[test]
    fn a_signed_in_answer_updates_the_jar_once() {
        let yt = client();
        assert!(!yt.take_changed(), "a jar we were given is not a change");
        yt.absorb_answer(yt.generation(), &[ROTATED.to_owned()], &page("1"));
        assert!(sidcc(&yt));
        assert!(yt.take_changed());
        assert!(!yt.take_changed());
    }

    #[test]
    fn a_guest_answer_does_not_touch_the_jar() {
        let yt = client();
        yt.absorb_answer(yt.generation(), &[ROTATED.to_owned()], &page("0"));
        assert!(!sidcc(&yt));
        assert!(!yt.take_changed());
    }

    #[test]
    fn an_answer_to_a_replaced_jar_is_dropped() {
        let yt = client();
        let sent_with = yt.generation();
        yt.set_cookies(Some(Cookies::parse(JAR).unwrap()));
        yt.absorb_answer(sent_with, &[ROTATED.to_owned()], &page("1"));
        assert!(!sidcc(&yt));
        assert!(!yt.take_changed());
    }

    #[test]
    fn an_answer_after_sign_out_brings_nothing_back() {
        let yt = client();
        let sent_with = yt.generation();
        yt.set_cookies(None);
        yt.absorb_answer(sent_with, &[ROTATED.to_owned()], &page("1"));
        assert!(yt.cookies().is_none());
    }

    #[test]
    fn reads_logged_in_flag() {
        let page = json!({"responseContext": {"serviceTrackingParams": [
            {"service": "GFEEDBACK", "params": [{"key": "logged_in", "value": "1"}]}]}});
        assert_eq!(logged_in(&page), Some(true));
        let guest = json!({"responseContext": {"serviceTrackingParams": [
            {"service": "GFEEDBACK", "params": [{"key": "logged_in", "value": "0"}]}]}});
        assert_eq!(logged_in(&guest), Some(false));
    }
}
