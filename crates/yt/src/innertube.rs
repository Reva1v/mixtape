//! Authenticated InnerTube client for music.youtube.com (`WEB_REMIX`): the same requests
//! the web player makes, signed with SAPISIDHASH from the user's cookies.

use crate::auth::{Cookies, ORIGIN};
use crate::parse::find_all;
use crate::{Result, YtError};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLIENT_NAME: &str = "WEB_REMIX";
const CLIENT_VERSION: &str = "1.20261013.01.00";
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

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
}

impl InnerTube {
    pub fn new(hl: &str, gl: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .gzip(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http, cookies: RwLock::new(None), hl: RwLock::new(hl.to_owned()), gl: gl.to_owned(), on_reach: OnceLock::new() })
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
        *self.cookies.write().unwrap_or_else(|e| e.into_inner()) = cookies.map(Arc::new);
    }

    pub fn cookies(&self) -> Option<Arc<Cookies>> {
        self.cookies.read().unwrap_or_else(|e| e.into_inner()).clone()
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
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
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
                let msg = resp.text().await.unwrap_or_default();
                let msg = serde_json::from_str::<Value>(&msg)
                    .ok()
                    .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_else(|| status.to_string());
                return Err(YtError::Network(format!("{endpoint}: {msg}")));
            }
        }
        resp.json().await.map_err(|e| YtError::Parse(format!("{endpoint}: {e}")))
    }

    pub async fn browse(&self, browse_id: &str) -> Result<Value> {
        self.call("browse", json!({ "browseId": browse_id })).await
    }

    pub async fn browse_params(&self, browse_id: &str, params: &str) -> Result<Value> {
        self.call("browse", json!({ "browseId": browse_id, "params": params })).await
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

/// `responseContext.serviceTrackingParams[*].params[logged_in]`.
pub fn logged_in(resp: &Value) -> Option<bool> {
    find_all(resp, "serviceTrackingParams").into_iter().find_map(|services| {
        services.as_array()?.iter().find_map(|s| {
            s.get("params")?.as_array()?.iter().find_map(|p| {
                (p.get("key")?.as_str()? == "logged_in").then(|| p.get("value").and_then(Value::as_str) == Some("1"))
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
