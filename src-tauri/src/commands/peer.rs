//! Stats sync with the phone (specs 007), the PC side: Settings → «Phone».
//!
//! The PC only answers. While a pairing exists or a pairing code is on screen it listens on
//! [`PORT`] on every interface, announces itself over mDNS, and hands each connection to
//! `mixtape_sync::peer::serve`. The phone dials in to pair, exchange plays or unpair.
//! Status changes reach the UI as `peer:status` ([`PeerStatus`]).

use super::{CmdResult, blocking};
use crate::state::AppState;
use mixtape_core::{AppError, ErrorKind, trf};
use mixtape_library::settings::{self, DEVICE_NAME};
use mixtape_library::{Db, peer_store};
use mixtape_sync::peer::mdns::Announcer;
use mixtape_sync::peer::{PORT, PairCode, Served, serve};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::async_runtime::JoinHandle;
use tauri::{Emitter, State};
use ts_rs::TS;

/// One connection gets this long, end to end (`serve` has no timeouts of its own).
const SERVE_TIMEOUT: Duration = Duration::from_secs(60);
/// `tailscale status --json` gets this long.
const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(3);
/// The Windows Firewall rule that lets the phone in.
const FIREWALL_RULE: &str = "Mixtape sync";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Whether Windows Firewall has [`FIREWALL_RULE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Firewall {
    Ok,
    Missing,
    /// Not checked yet (it is on the first pairing), or `netsh` didn't answer.
    Unknown,
}

/// What Settings → «Phone» shows; also the `peer:status` event.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerStatus {
    pub paired: bool,
    #[ts(optional)]
    pub peer_name: Option<String>,
    /// Unix ms.
    #[ts(optional, type = "number")]
    pub paired_at: Option<i64>,
    /// The last finished exchange, Unix ms.
    #[ts(optional, type = "number")]
    pub last_exchange: Option<i64>,
    /// The server is up (a pairing exists or a code is shown).
    pub listening: bool,
    pub firewall: Firewall,
}

/// A pairing code to show.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PairStart {
    /// `mixtape://pair?…`: what the QR code holds.
    pub uri: String,
    /// The QR code as an SVG document (black on white, with its quiet zone).
    pub svg: String,
    /// Unix ms.
    #[ts(type = "number")]
    pub expires_at: i64,
}

/// The listener and the mDNS announcement, both stopped on drop.
struct Server {
    task: JoinHandle<()>,
    _announcer: Option<Announcer>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
struct Inner {
    /// The code on screen; dropped once a phone paired with it or the screen closed.
    code: Option<PairCode>,
    server: Option<Server>,
    firewall: Option<Firewall>,
}

/// The PC side of the phone sync, in [`AppState`].
#[derive(Default)]
pub struct PeerState {
    inner: Mutex<Inner>,
}

impl PeerState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// At start: names this PC if it has no name yet and starts the server when paired.
pub fn spawn(st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let db = st.db.clone();
        if let Err(e) = blocking(move || ensure_device_name(&db)).await {
            tracing::warn!(error = %e, "device name not saved");
        }
        reconcile(&st);
    });
}

/// Saves the computer's name as this device's name when none is set, so the phone shows
/// the same name later even if the computer is renamed.
fn ensure_device_name(db: &Db) -> CmdResult<()> {
    if settings::get::<String>(db, DEVICE_NAME)?.is_none() {
        settings::set(db, DEVICE_NAME, &settings::default_device_name())?;
    }
    Ok(())
}

fn pairing(st: &AppState) -> Option<peer_store::Pairing> {
    st.db
        .read(peer_store::pairing)
        .inspect_err(|e| tracing::warn!(error = %e, "pairing unreadable"))
        .ok()
        .flatten()
}

fn status(st: &AppState) -> PeerStatus {
    let p = pairing(st);
    let inner = st.peer.lock();
    PeerStatus {
        paired: p.is_some(),
        peer_name: p.as_ref().map(|p| p.peer_name.clone()),
        paired_at: p.as_ref().map(|p| p.paired_at),
        last_exchange: p.as_ref().and_then(|p| p.last_exchange),
        listening: inner.server.is_some(),
        firewall: inner.firewall.unwrap_or(Firewall::Unknown),
    }
}

fn emit_status(st: &AppState) {
    let _ = st.app.emit("peer:status", status(st));
}

/// Starts or stops the server: it runs while a pairing exists or a code is shown. Must be
/// called on the async runtime (the listener is registered with it).
fn reconcile(st: &Arc<AppState>) {
    let paired = pairing(st).is_some();
    let mut inner = st.peer.lock();
    let wanted = paired || inner.code.is_some();
    if !wanted {
        if inner.server.take().is_some() {
            tracing::info!("phone sync server stopped");
        }
        return;
    }
    if inner.server.is_some() {
        return;
    }
    match start_server(st) {
        Ok(server) => {
            tracing::info!(port = PORT, "phone sync server listening");
            inner.server = Some(server);
        }
        Err(e) => tracing::warn!(error = %e, port = PORT, "phone sync server not started"),
    }
}

fn start_server(st: &Arc<AppState>) -> std::io::Result<Server> {
    let std_listener = std::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)))?;
    std_listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(std_listener)?;
    let announcer = st
        .db
        .read(|c| Ok((settings::device_id(c)?, settings::device_name(c)?)))
        .map_err(|e| e.to_string())
        .and_then(|(id, name)| Announcer::start(&id, &name, PORT).map_err(|e| e.to_string()))
        .inspect_err(|e| tracing::warn!(error = %e, "mDNS announcement unavailable"))
        .ok();
    let st = st.clone();
    let task = tauri::async_runtime::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, from)) => {
                    let st = st.clone();
                    tauri::async_runtime::spawn(async move { answer(st, stream, from).await });
                }
                Err(e) => {
                    tracing::debug!(error = %e, "phone sync accept failed");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    });
    Ok(Server { task, _announcer: announcer })
}

/// One connection from the phone.
async fn answer(st: Arc<AppState>, stream: tokio::net::TcpStream, from: SocketAddr) {
    let _ = stream.set_nodelay(true);
    // `serve` refuses an expired code itself, telling the phone to ask for a new one.
    let code = st.peer.lock().code.clone();
    let served = match tokio::time::timeout(SERVE_TIMEOUT, serve(stream, &st.db, code.as_ref())).await {
        Ok(r) => r,
        Err(_) => {
            tracing::info!(%from, "phone sync timed out");
            return;
        }
    };
    match served {
        Ok(Served::Paired(p)) => {
            tracing::info!(%from, peer = %p.peer_name, "phone paired");
            // `serve` doesn't spend the code: drop it so it can't pair a second phone.
            let mut inner = st.peer.lock();
            if inner.code.as_ref().is_some_and(|c| code.as_ref().is_some_and(|used| used.key == c.key)) {
                inner.code = None;
            }
        }
        Ok(Served::Exchanged(r)) => {
            tracing::info!(%from, received = r.received, sent = r.sent, "phone sync exchange");
            if r.received > 0 {
                let _ = st.app.emit("stats:changed", ());
            }
        }
        Ok(Served::Unpaired) => tracing::info!(%from, "phone unpaired"),
        Err(e) => {
            tracing::info!(%from, error = %e, "phone sync connection failed");
            return;
        }
    }
    reconcile(&st);
    emit_status(&st);
}

// ---- Commands ----

#[tauri::command]
pub async fn peer_status(state: State<'_, Arc<AppState>>) -> CmdResult<PeerStatus> {
    let st = state.inner().clone();
    blocking(move || Ok(status(&st))).await
}

/// Shows a new pairing code (good for 5 minutes) and starts the server for it.
#[tauri::command]
pub async fn peer_pair_start(state: State<'_, Arc<AppState>>) -> CmdResult<PairStart> {
    let st = state.inner().clone();
    let db = st.db.clone();
    let name = blocking(move || {
        ensure_device_name(&db)?;
        Ok(db.read(settings::device_name)?)
    })
    .await?;
    let addresses = addresses().await;
    tracing::debug!(?addresses, "pairing code addresses");
    let code = PairCode::new(name, PORT, addresses, mixtape_library::repo::now_ms().div_euclid(1000))?;
    let uri = code.to_uri();
    let expires_at = code.expires_at * 1000;
    st.peer.lock().code = Some(code);
    reconcile(&st);
    if st.peer.lock().server.is_none() {
        st.peer.lock().code = None;
        return Err(AppError::new(
            ErrorKind::Network,
            trf!(
                "Port {} is busy: another program or a second Mixtape is using it",
                "Порт {} зайнятий: його використовує інша програма або друга копія Mixtape",
                "Порт {} занят: его использует другая программа или вторая копия Mixtape",
                PORT
            ),
        ));
    }
    if st.peer.lock().firewall != Some(Firewall::Ok) {
        let fw = firewall_check().await;
        st.peer.lock().firewall = Some(fw);
    }
    let svg = qr_svg(&uri)?;
    emit_status(&st);
    Ok(PairStart { uri, svg, expires_at })
}

/// The pairing screen closed: the code is dropped (and the server stops if not paired).
#[tauri::command]
pub async fn peer_pair_cancel(state: State<'_, Arc<AppState>>) -> CmdResult<PeerStatus> {
    let st = state.inner().clone();
    st.peer.lock().code = None;
    reconcile(&st);
    let s = status(&st);
    let _ = st.app.emit("peer:status", s.clone());
    Ok(s)
}

/// Forgets the phone here. The phone finds out on its next exchange (the PC refuses it).
#[tauri::command]
pub async fn peer_unpair(state: State<'_, Arc<AppState>>) -> CmdResult<PeerStatus> {
    let st = state.inner().clone();
    st.db.write(|t| peer_store::remove_pairing(t))?;
    reconcile(&st);
    let s = status(&st);
    let _ = st.app.emit("peer:status", s.clone());
    Ok(s)
}

/// Adds the firewall rule through UAC (Windows asks the user; nothing happens silently).
#[tauri::command]
pub async fn peer_firewall_add(state: State<'_, Arc<AppState>>) -> CmdResult<PeerStatus> {
    let st = state.inner().clone();
    blocking(add_firewall_rule).await?;
    let fw = firewall_check().await;
    st.peer.lock().firewall = Some(fw);
    let s = status(&st);
    let _ = st.app.emit("peer:status", s.clone());
    Ok(s)
}

// ---- The pairing code ----

fn qr_svg(uri: &str) -> CmdResult<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::with_error_correction_level(uri, qrcode::EcLevel::M).map_err(|e| {
        AppError::new(ErrorKind::Internal, trf!("QR code failed: {e}", "QR-код не вдався: {e}", "QR-код не получился: {e}"))
    })?;
    let svg = code
        .render::<svg::Color<'_>>()
        .module_dimensions(1, 1)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    // Inline in the page: no XML declaration.
    Ok(match svg.find("<svg") {
        Some(i) => svg[i..].to_owned(),
        None => svg,
    })
}

/// Where the phone can reach this PC, best first: LAN addresses, then Tailscale ones
/// (`100.64.0.0/10`), then the Tailscale MagicDNS name.
async fn addresses() -> Vec<String> {
    let mut ips: Vec<Ipv4Addr> = match if_addrs::get_if_addrs() {
        Ok(ifs) => ifs
            .iter()
            .filter(|i| i.is_oper_up())
            .filter_map(|i| match i.ip() {
                IpAddr::V4(ip) => Some(ip),
                IpAddr::V6(_) => None,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "network interfaces unreadable");
            Vec::new()
        }
    };
    let tailscale = tailscale_self().await;
    if let Some((_, ts_ips)) = &tailscale {
        ips.extend(ts_ips);
    }
    let mut out = order_addresses(ips);
    if let Some((Some(dns), _)) = tailscale
        && !out.contains(&dns)
    {
        out.push(dns);
    }
    out
}

fn is_tailscale(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..128).contains(&b)
}

/// Drops loopback, link-local and unspecified addresses and duplicates; LAN before Tailscale,
/// private ranges before the rest.
fn order_addresses(ips: Vec<Ipv4Addr>) -> Vec<String> {
    let mut ips: Vec<Ipv4Addr> = ips
        .into_iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() && !ip.is_multicast())
        .collect();
    let rank = |ip: &Ipv4Addr| match () {
        _ if is_tailscale(*ip) => 2,
        _ if ip.is_private() => 0,
        _ => 1,
    };
    // Stable: interface order within a rank.
    ips.sort_by_key(rank);
    let mut out: Vec<String> = Vec::new();
    for ip in ips {
        let s = ip.to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// The MagicDNS name (without the trailing dot) and the IPv4 addresses of this machine in
/// Tailscale, when `tailscale status --json` answers.
async fn tailscale_self() -> Option<(Option<String>, Vec<Ipv4Addr>)> {
    let mut candidates = vec![std::path::PathBuf::from("tailscale")];
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(std::path::Path::new(&pf).join("Tailscale").join("tailscale.exe"));
    }
    for exe in candidates {
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args(["status", "--json"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = match tokio::time::timeout(TAILSCALE_TIMEOUT, cmd.output()).await {
            Ok(Ok(out)) => out,
            // Not on this path: try the next one.
            Ok(Err(_)) => continue,
            Err(_) => return None,
        };
        if !out.status.success() {
            return None;
        }
        return parse_tailscale(&out.stdout);
    }
    None
}

fn parse_tailscale(json: &[u8]) -> Option<(Option<String>, Vec<Ipv4Addr>)> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let me = v.get("Self")?;
    let dns = me
        .get("DNSName")
        .and_then(|d| d.as_str())
        .map(|d| d.trim_end_matches('.').to_owned())
        .filter(|d| !d.is_empty());
    let ips = me
        .get("TailscaleIPs")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()?.parse::<Ipv4Addr>().ok()).collect())
        .unwrap_or_default();
    Some((dns, ips))
}

// ---- Windows Firewall ----

/// Whether the rule exists (`netsh` exits 0 when it lists it, 1 when no rule matches).
async fn firewall_check() -> Firewall {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("netsh");
        cmd.raw_arg(format!("advfirewall firewall show rule name=\"{FIREWALL_RULE}\""))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW);
        match tokio::time::timeout(Duration::from_secs(5), cmd.status()).await {
            Ok(Ok(s)) if s.success() => Firewall::Ok,
            Ok(Ok(_)) => Firewall::Missing,
            _ => Firewall::Unknown,
        }
    }
    #[cfg(not(windows))]
    Firewall::Unknown
}

/// Runs `netsh advfirewall firewall add rule …` elevated: Windows shows its UAC prompt and
/// the user decides. Waits for netsh to finish.
fn add_firewall_rule() -> CmdResult<()> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
        use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
        use windows::core::{HSTRING, w};

        let params = HSTRING::from(format!(
            "advfirewall firewall add rule name=\"{FIREWALL_RULE}\" dir=in action=allow protocol=TCP localport={PORT} profile=private"
        ));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: w!("netsh.exe"),
            lpParameters: windows::core::PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // SAFETY: `info` and the strings it points to outlive the call; the process handle
        // it returns is closed below.
        let started = unsafe { ShellExecuteExW(&mut info) };
        if started.is_err() || info.hProcess.is_invalid() {
            // Mostly «No» in the UAC prompt (ERROR_CANCELLED).
            return Err(AppError::new(
                ErrorKind::Invalid,
                trf!(
                    "The firewall rule wasn't added",
                    "Правило брандмауера не додано",
                    "Правило брандмауэра не добавлено"
                ),
            ));
        }
        let mut code = 1u32;
        // SAFETY: `hProcess` is a valid process handle owned here.
        unsafe {
            if WaitForSingleObject(info.hProcess, 60_000) == WAIT_OBJECT_0 {
                let _ = GetExitCodeProcess(info.hProcess, &mut code);
            }
            let _ = CloseHandle(info.hProcess);
        }
        if code != 0 {
            return Err(AppError::new(
                ErrorKind::Internal,
                trf!(
                    "netsh couldn't add the firewall rule (code {})",
                    "netsh не зміг додати правило брандмауера (код {})",
                    "netsh не смог добавить правило брандмауэра (код {})",
                    code
                ),
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    Err(AppError::new(ErrorKind::Invalid, "Windows only"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_ordered_and_filtered() {
        let ips = ["100.101.1.2", "127.0.0.1", "169.254.3.4", "192.168.1.5", "10.0.0.7", "8.8.8.8", "192.168.1.5"]
            .map(|s| s.parse().unwrap())
            .to_vec();
        assert_eq!(order_addresses(ips), ["192.168.1.5", "10.0.0.7", "8.8.8.8", "100.101.1.2"]);
        assert!(!is_tailscale("100.63.0.1".parse().unwrap()));
        assert!(!is_tailscale("100.128.0.1".parse().unwrap()));
    }

    #[test]
    fn tailscale_json() {
        let json = br#"{"Self":{"DNSName":"desk.tail1234.ts.net.","TailscaleIPs":["100.90.1.2","fd7a:115c::1"]}}"#;
        let (dns, ips) = parse_tailscale(json).unwrap();
        assert_eq!(dns.as_deref(), Some("desk.tail1234.ts.net"));
        assert_eq!(ips, vec!["100.90.1.2".parse::<Ipv4Addr>().unwrap()]);
        assert!(parse_tailscale(b"not json").is_none());
    }

    #[test]
    fn qr_is_inline_svg() {
        let svg = qr_svg("mixtape://pair?v=1&k=abc").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("#ffffff"));
    }
}
