//! Direct audio URLs without yt-dlp, for Android: InnerTube `player` requests as clients
//! whose answers carry ready URLs (no signature to decipher, no PO token), tried in order.
//!
//! The client list is data ([`DEFAULT_CLIENTS`], `clients.json`), so a client YouTube breaks
//! is replaced without touching the logic. The clients and their parameters come from
//! ReVanced's "Spoof video streams" patch (GPL-3.0, see THIRD-PARTY-NOTICES).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use mixtape_core::{TrackId, tr};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::innertube::{MAX_JSON_BYTES, read_limited};
use crate::resolver::{Container, Resolve, ResolveFuture, ResolvedStream, parse_expire};
use crate::{Result, YtError};

/// The built-in client list, best first.
pub const DEFAULT_CLIENTS: &str = include_str!("clients.json");

const API: &str = "https://youtubei.googleapis.com/youtubei/v1";
/// Audio formats in order of preference: Opus 160k, AAC 128k, then the smaller ones.
const ITAGS: [u64; 5] = [251, 140, 250, 249, 139];
/// Lifetime assumed when the URL carries no `expire=`.
const FALLBACK_TTL_SECS: u64 = 5 * 60 * 60;

/// One InnerTube client to pose as.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Client {
    /// For logs only.
    pub name: String,
    pub client_name: String,
    /// `X-YouTube-Client-Name`.
    pub client_id: u32,
    pub client_version: String,
    pub user_agent: String,
    pub device_make: String,
    pub device_model: String,
    pub os_name: String,
    pub os_version: String,
    #[serde(default)]
    pub android_sdk_version: Option<String>,
    pub endpoint: Endpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Endpoint {
    /// `player`, answers the player response itself.
    Player,
    /// `reel/reel_item_watch`, answers it under `playerResponse`.
    Reel,
}

/// Parses a client list such as [`DEFAULT_CLIENTS`].
pub fn parse_clients(json: &str) -> Result<Vec<Client>> {
    serde_json::from_str(json).map_err(|e| YtError::Parse(e.to_string()))
}

/// Resolves video ids with the clients in order, caching the URLs in memory.
pub struct DirectResolver {
    http: reqwest::Client,
    clients: Vec<Client>,
    api: String,
    hl: String,
    gl: String,
    visitor_data: RwLock<Option<String>>,
    cache: Mutex<HashMap<TrackId, ResolvedStream>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl DirectResolver {
    pub fn new(http: reqwest::Client, clients: Vec<Client>, hl: &str, gl: &str) -> Self {
        Self {
            http,
            clients,
            api: API.to_owned(),
            hl: hl.to_owned(),
            gl: gl.to_owned(),
            visitor_data: RwLock::new(None),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Sends the requests to `api` instead of YouTube (tests).
    pub fn with_api(mut self, api: impl Into<String>) -> Self {
        self.api = api.into();
        self
    }

    /// The `visitorData` of an earlier InnerTube answer: without it these clients are more
    /// often asked to prove they are not a bot.
    pub fn set_visitor_data(&self, v: Option<String>) {
        *self.visitor_data.write().unwrap_or_else(|e| e.into_inner()) = v;
    }

    pub fn has_visitor_data(&self) -> bool {
        self.visitor_data
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// A cached URL that is still fresh.
    pub fn cached(&self, id: &TrackId) -> Option<ResolvedStream> {
        lock(&self.cache)
            .get(id)
            .filter(|s| s.is_fresh(now_unix()))
            .cloned()
    }

    /// The stream of `id`, plus the name of the client that gave it.
    pub async fn resolve_with_client(&self, id: &TrackId) -> Result<(ResolvedStream, String)> {
        let mut last: Option<YtError> = None;
        for client in &self.clients {
            match self.ask(client, id).await {
                Ok(s) => {
                    lock(&self.cache).insert(id.clone(), s.clone());
                    return Ok((s, client.name.clone()));
                }
                Err(e) => {
                    tracing::debug!(client = %client.name, %id, "no stream: {e}");
                    // Keep the most telling reason; on a tie, the first client's.
                    if last.as_ref().is_none_or(|l| telling(&e) > telling(l)) {
                        last = Some(e);
                    }
                }
            }
        }
        Err(last.unwrap_or_else(no_direct_url))
    }

    async fn ask(&self, client: &Client, id: &TrackId) -> Result<ResolvedStream> {
        let visitor = self
            .visitor_data
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let body = request_body(client, id, &self.hl, &self.gl, visitor.as_deref());
        let path = match client.endpoint {
            Endpoint::Player => "player",
            Endpoint::Reel => "reel/reel_item_watch",
        };
        let mut req = self
            .http
            .post(format!("{}/{path}?prettyPrint=false", self.api))
            .header(reqwest::header::USER_AGENT, &client.user_agent)
            .header("X-YouTube-Client-Name", client.client_id.to_string())
            .header("X-YouTube-Client-Version", &client.client_version)
            .json(&body);
        if let Some(v) = &visitor {
            req = req.header("X-Goog-Visitor-Id", v);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.as_u16() == 429 {
            return Err(YtError::RateLimited);
        }
        let bytes = read_limited(resp, MAX_JSON_BYTES)
            .await
            .map_err(|e| YtError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(YtError::Network(format!("HTTP {status}")));
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| YtError::Parse(e.to_string()))?;
        let player = match client.endpoint {
            Endpoint::Player => &v,
            Endpoint::Reel => v.get("playerResponse").unwrap_or(&v),
        };
        parse_player(player, now_unix())
    }
}

impl Resolve for DirectResolver {
    fn resolve_one<'a>(&'a self, id: &'a TrackId) -> ResolveFuture<'a> {
        Box::pin(async move {
            if let Some(s) = self.cached(id) {
                return Ok(s);
            }
            self.resolve_with_client(id).await.map(|(s, _)| s)
        })
    }

    fn invalidate(&self, id: &TrackId) {
        lock(&self.cache).remove(id);
    }
}

fn request_body(client: &Client, id: &TrackId, hl: &str, gl: &str, visitor: Option<&str>) -> Value {
    let mut c = json!({
        "clientName": client.client_name,
        "clientVersion": client.client_version,
        "deviceMake": client.device_make,
        "deviceModel": client.device_model,
        "osName": client.os_name,
        "osVersion": client.os_version,
        "hl": hl,
        "gl": gl,
    });
    if let Some(sdk) = &client.android_sdk_version {
        c["androidSdkVersion"] = json!(sdk);
    }
    if let Some(v) = visitor {
        c["visitorData"] = json!(v);
    }
    let context = json!({ "client": c });
    match client.endpoint {
        Endpoint::Player => json!({
            "context": context,
            "videoId": id.as_str(),
            "contentCheckOk": true,
            "racyCheckOk": true,
        }),
        Endpoint::Reel => json!({
            "context": context,
            "playerRequest": { "videoId": id.as_str(), "contentCheckOk": true, "racyCheckOk": true },
            "disablePlayerResponse": false,
        }),
    }
}

fn no_direct_url() -> YtError {
    YtError::Unavailable(
        tr(
            "YouTube gave no playable stream for this track",
            "YouTube не віддав потік для цього треку",
            "YouTube не отдал поток для этого трека",
        )
        .to_owned(),
    )
}

/// How much an error says about the track: YouTube's own reason it cannot be played beats
/// a bot check, which beats a network error, which beats "no direct URL".
fn telling(e: &YtError) -> u8 {
    match e {
        YtError::Unavailable(r) if *r != no_direct_url().to_string() => 3,
        YtError::RateLimited => 2,
        YtError::Network(_) => 1,
        _ => 0,
    }
}

fn is_bot_check(reason: &str) -> bool {
    let r = reason.to_lowercase();
    r.contains("not a bot") || r.contains("confirm you")
}

/// The best audio stream of a player response, at `now` (unix seconds).
pub fn parse_player(v: &Value, now: u64) -> Result<ResolvedStream> {
    let ps = &v["playabilityStatus"];
    let status = ps["status"].as_str().unwrap_or("");
    if status != "OK" {
        let reason = ps["reason"]
            .as_str()
            .or_else(|| ps["messages"][0].as_str())
            .unwrap_or(status)
            .to_owned();
        return Err(if is_bot_check(&reason) {
            YtError::RateLimited
        } else if reason.is_empty() {
            no_direct_url()
        } else {
            YtError::Unavailable(reason)
        });
    }
    let formats = v["streamingData"]["adaptiveFormats"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let audio: Vec<&Value> = formats
        .iter()
        .filter(|f| f["url"].is_string())
        .filter(|f| container(f).is_some())
        .collect();
    let rank = |f: &Value| {
        let itag = f["itag"].as_u64().unwrap_or(0);
        ITAGS.iter().position(|&t| t == itag).unwrap_or(ITAGS.len())
    };
    let best = audio
        .into_iter()
        .min_by_key(|f| rank(f))
        .ok_or_else(no_direct_url)?;
    let url = best["url"].as_str().unwrap_or_default().to_owned();
    let num = |x: &Value| {
        x.as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| x.as_u64())
    };
    let duration_ms = num(&best["approxDurationMs"])
        .or_else(|| num(&v["videoDetails"]["lengthSeconds"]).map(|s| s * 1000))
        .unwrap_or(0);
    Ok(ResolvedStream {
        expires_at: parse_expire(&url).unwrap_or(now + FALLBACK_TTL_SECS),
        itag: best["itag"].as_u64().unwrap_or(0) as u32,
        container: container(best).unwrap_or(Container::Webm),
        size: num(&best["contentLength"]),
        duration_ms,
        url,
    })
}

fn container(f: &Value) -> Option<Container> {
    let mime = f["mimeType"].as_str()?;
    if mime.starts_with("audio/webm") {
        Some(Container::Webm)
    } else if mime.starts_with("audio/mp4") {
        Some(Container::M4a)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn built_in_clients_parse() {
        let clients = parse_clients(DEFAULT_CLIENTS).unwrap();
        assert!(clients.len() >= 3);
        assert_eq!(clients[0].client_name, "ANDROID_VR");
        assert!(clients.iter().any(|c| c.endpoint == Endpoint::Reel));
    }

    #[test]
    fn picks_opus_with_expiry_and_size() {
        let s = parse_player(&fixture("player_ok.json"), 0).unwrap();
        assert_eq!(s.itag, 251);
        assert_eq!(s.container, Container::Webm);
        assert_eq!(s.expires_at, 1761170000);
        assert_eq!(s.size, Some(3_456_789));
        assert_eq!(s.duration_ms, 213_041);
        assert!(s.url.contains("itag=251"));
    }

    #[test]
    fn cipher_only_is_no_direct_url() {
        let e = parse_player(&fixture("player_cipher.json"), 0).unwrap_err();
        assert!(matches!(e, YtError::Unavailable(_)));
    }

    #[test]
    fn login_required_is_unavailable_with_reason() {
        match parse_player(&fixture("player_unplayable.json"), 0) {
            Err(YtError::Unavailable(r)) => assert!(r.contains("private")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bot_check_is_rate_limited() {
        let e = parse_player(&fixture("player_bot.json"), 0).unwrap_err();
        assert!(matches!(e, YtError::RateLimited));
    }

    #[test]
    fn reel_body_nests_the_player_request() {
        let clients = parse_clients(DEFAULT_CLIENTS).unwrap();
        let reel = clients
            .iter()
            .find(|c| c.endpoint == Endpoint::Reel)
            .unwrap();
        let b = request_body(reel, &TrackId::new("dQw4w9WgXcQ"), "en", "US", Some("vd"));
        assert_eq!(b["playerRequest"]["videoId"], "dQw4w9WgXcQ");
        assert_eq!(b["context"]["client"]["visitorData"], "vd");
    }

    /// Answers each request with the next body of `bodies` (the last one repeats).
    async fn serve(bodies: Vec<Value>) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let n = count.fetch_add(1, Ordering::SeqCst);
                let body = bodies[n.min(bodies.len() - 1)].to_string();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let _ = sock.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}"), hits)
    }

    #[tokio::test]
    async fn falls_through_to_the_next_client_and_caches() {
        let (api, hits) = serve(vec![
            fixture("player_cipher.json"),
            fixture("player_ok.json"),
        ])
        .await;
        let clients = parse_clients(DEFAULT_CLIENTS).unwrap();
        let second = clients[1].name.clone();
        let r = DirectResolver::new(reqwest::Client::new(), clients, "en", "US").with_api(api);
        let id = TrackId::new("dQw4w9WgXcQ");
        let (s, client) = r.resolve_with_client(&id).await.unwrap();
        assert_eq!(s.itag, 251);
        assert_eq!(client, second);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        // The fixture's URL expires in the past, so it is not served from the cache.
        assert!(r.cached(&id).is_none());
    }

    #[tokio::test]
    async fn every_client_failing_keeps_the_unavailable_reason() {
        let (api, hits) = serve(vec![
            fixture("player_unplayable.json"),
            fixture("player_cipher.json"),
        ])
        .await;
        let clients = parse_clients(DEFAULT_CLIENTS).unwrap();
        let n = clients.len();
        let r = DirectResolver::new(reqwest::Client::new(), clients, "en", "US").with_api(api);
        match r.resolve_one(&TrackId::new("dQw4w9WgXcQ")).await {
            Err(YtError::Unavailable(reason)) => assert!(reason.contains("private")),
            other => panic!("{other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), n);
    }
}
