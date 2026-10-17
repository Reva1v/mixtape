//! Live check of YouTube's loudness values: `cargo run -p mixtape-yt --example loudness_probe --
//! <cookies.txt> <videoId>…`. One `player` request per id; prints only the playability status
//! and the loudness fields, never cookie values.

use mixtape_core::TrackId;
use mixtape_yt::auth::Cookies;
use mixtape_yt::innertube::InnerTube;
use mixtape_yt::loudness::parse_loudness;
use serde_json::Value;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: loudness_probe <cookies.txt> <videoId>…")?;
    let yt = InnerTube::new("en", "US")?;
    yt.set_cookies(Some(Cookies::parse(&std::fs::read_to_string(path)?)?));
    for id in args {
        let resp = yt.player_meta(&TrackId::new(id.as_str())).await?;
        let status = resp
            .pointer("/playabilityStatus/status")
            .and_then(Value::as_str);
        let audio = resp.pointer("/playerConfig/audioConfig");
        println!(
            "{id}: status={status:?}\n  audioConfig={audio:?}\n  parsed: {:?} LUFS",
            parse_loudness(&resp)
        );
    }
    Ok(())
}
