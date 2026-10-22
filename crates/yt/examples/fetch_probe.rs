//! Does a resolved stream URL hand out bytes, and how fast?
//! `cargo run -p mixtape-yt --example fetch_probe -- <videoId>…`.
//! Resolves each id like the app does (visitorData first), asks googlevideo for the first
//! 64 KiB with a plain client and with the resolving client's User-Agent, then times the whole
//! file in one request and in 1 MiB ranges. Reads bytes only; plays nothing.

use mixtape_core::TrackId;
use mixtape_yt::direct::{DEFAULT_CLIENTS, DirectResolver, parse_clients};
use mixtape_yt::innertube::InnerTube;
use std::time::Instant;

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
        let (s, client) = match r.resolve_with_client(&id).await {
            Ok(x) => x,
            Err(e) => {
                println!("{id}: no stream: {e}");
                continue;
            }
        };
        let ua = clients.iter().find(|c| c.name == client).map(|c| c.user_agent.clone()).unwrap_or_default();
        for (label, req) in [
            ("plain", plain.get(&s.url)),
            ("client UA", plain.get(&s.url).header(reqwest::header::USER_AGENT, &ua)),
        ] {
            let resp = req.header(reqwest::header::RANGE, "bytes=0-65535").send().await?;
            let status = resp.status();
            let n = resp.bytes().await.map(|b| b.len()).unwrap_or(0);
            println!("{id} via {client} itag {} [{label}]: HTTP {status}, {n} bytes", s.itag);
        }
        let size = s.size.unwrap_or(0);
        if size > 0 {
            let t = Instant::now();
            let mut got = 0usize;
            let mut start = 0u64;
            while start < size {
                let end = (start + (1 << 20) - 1).min(size - 1);
                let t1 = Instant::now();
                let b = it.http().get(&s.url).header(reqwest::header::RANGE, format!("bytes={start}-{end}")).send().await?.bytes().await?;
                println!("    range {start}-{end}: {} bytes in {:.2} s", b.len(), t1.elapsed().as_secs_f64());
                got += b.len();
                start = end + 1;
            }
            let secs = t.elapsed().as_secs_f64();
            println!("  in 1 MiB ranges: {got} bytes in {secs:.1} s ({:.0} KiB/s)", got as f64 / 1024.0 / secs);
        }
        for (start, end) in [(0u64, (1u64 << 20) - 1), (1 << 20, (2 << 20) - 1), (0, (10u64 << 20) - 1), (0, size.saturating_sub(1))] {
            let t1 = Instant::now();
            let url = format!("{}&range={start}-{end}", s.url);
            match tokio::time::timeout(std::time::Duration::from_secs(20), async { it.http().get(&url).send().await?.bytes().await }).await {
                Ok(Ok(b)) => println!("    range param {start}-{end}: {} bytes in {:.2} s", b.len(), t1.elapsed().as_secs_f64()),
                Ok(Err(e)) => println!("    range param {start}-{end}: error {e}"),
                Err(_) => println!("    range param {start}-{end}: timed out after 20 s"),
            }
        }
        let t = Instant::now();
        match it.http().get(&s.url).send().await {
            Ok(resp) => match resp.bytes().await {
                Ok(b) => {
                    let secs = t.elapsed().as_secs_f64();
                    println!("  whole file in one request: {} bytes in {secs:.1} s ({:.0} KiB/s)", b.len(), b.len() as f64 / 1024.0 / secs);
                }
                Err(e) => println!("  whole file in one request: failed after {:.1} s: {e}", t.elapsed().as_secs_f64()),
            },
            Err(e) => println!("  whole file in one request: failed: {e}"),
        }
    }
    Ok(())
}
