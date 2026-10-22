//! Does a resolved stream URL hand out bytes? `cargo run -p mixtape-yt --example fetch_probe -- <videoId>…`.
//! Resolves each id like the app does (visitorData first), then asks googlevideo for the first
//! 64 KiB with a plain client and with the resolving client's User-Agent, printing the HTTP
//! status and byte count of each. Reads only those bytes; plays nothing.

use mixtape_core::TrackId;
use mixtape_yt::direct::{DEFAULT_CLIENTS, DirectResolver, parse_clients};
use mixtape_yt::innertube::InnerTube;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    let it = InnerTube::new("ru", "UA")?;
    let clients = parse_clients(DEFAULT_CLIENTS)?;
    let plain = reqwest::Client::new();
    for raw in ids {
        let id = TrackId::new(raw.as_str());
        let info = it.track_info(&id).await?;
        let r = DirectResolver::new(it.http().clone(), clients.clone(), "ru", "UA");
        r.set_visitor_data(info.visitor_data.clone());
        let (s, client) = r.resolve_with_client(&id).await?;
        let ua = clients.iter().find(|c| c.name == client).map(|c| c.user_agent.clone()).unwrap_or_default();
        for (label, req) in [
            ("plain", plain.get(&s.url)),
            ("app http", it.http().get(&s.url)),
            ("client UA", plain.get(&s.url).header(reqwest::header::USER_AGENT, &ua)),
        ] {
            let resp = req.header(reqwest::header::RANGE, "bytes=0-65535").send().await?;
            let status = resp.status();
            let n = resp.bytes().await.map(|b| b.len()).unwrap_or(0);
            println!("{id} via {client} [{label}]: HTTP {status}, {n} bytes");
        }
    }
    Ok(())
}
