//! Finding the PC on the local network: it announces `_mixtape._tcp` with its device id in
//! the TXT record, the phone browses for the id it is paired with. mDNS does not cross
//! Tailscale; there the phone dials the code's addresses instead. On Android the app must
//! hold a `WifiManager.MulticastLock` while browsing, or no answers arrive.

use super::PeerError;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// The service type.
pub const SERVICE: &str = "_mixtape._tcp.local.";

fn mdns_err(e: mdns_sd::Error) -> PeerError {
    PeerError::Io(std::io::Error::other(e.to_string()))
}

/// The PC's announcement; withdrawn on drop.
pub struct Announcer {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Announcer {
    /// Announces this device (`device` = `settings.device_id`) on every interface at `port`.
    pub fn start(device: &str, name: &str, port: u16) -> Result<Self, PeerError> {
        let daemon = ServiceDaemon::new().map_err(mdns_err)?;
        let props = [("device", device), ("name", name), ("v", "1")];
        let info = ServiceInfo::new(SERVICE, device, &format!("{device}.local."), "", port, &props[..])
            .map_err(mdns_err)?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_owned();
        daemon.register(info).map_err(mdns_err)?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Announcer {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Browses for the device `peer_device` for up to `timeout` and returns its IPv4 addresses
/// (empty when it didn't answer). Blocks: run it off the async threads.
pub fn find(peer_device: &str, timeout: Duration) -> Result<Vec<SocketAddr>, PeerError> {
    let daemon = ServiceDaemon::new().map_err(mdns_err)?;
    let events = daemon.browse(SERVICE).map_err(mdns_err)?;
    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(left) else {
            break;
        };
        if let ServiceEvent::ServiceResolved(s) = event
            && s.get_property_val_str("device") == Some(peer_device)
        {
            let port = s.get_port();
            found = s.get_addresses_v4().into_iter().map(|ip| SocketAddr::new(IpAddr::V4(ip), port)).collect();
            found.sort();
            break;
        }
    }
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcer_starts_and_stops() {
        // Needs a network interface; none in some sandboxes, where the daemon still starts.
        let a = Announcer::start("00000000-0000-4000-8000-000000000000", "Test PC", 47821).unwrap();
        assert!(a.fullname.starts_with("00000000-0000-4000-8000-000000000000."));
        drop(a);
    }
}
