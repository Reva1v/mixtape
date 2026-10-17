//! A tiny local HTTP server for download tests: serves one body with the range behaviours of
//! googlevideo and of ordinary servers, and logs the requested ranges.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Copy)]
pub enum Mode {
    /// 200 with only the requested range, like googlevideo with `&range=`.
    Param,
    /// 206 with `Content-Range`, like a standard server.
    Partial,
    /// Like `Partial`, but the body trickles out in 4 KiB pieces with this pause between them.
    Slow(Duration),
    /// Ignores ranges and always sends the whole file.
    Whole,
    Forbidden,
    /// Sends the headers and `n` bytes of the piece, then hangs.
    Stall(usize),
}

pub struct Server {
    /// The body's URL.
    pub base: String,
    /// A URL that always answers 403, like an expired googlevideo link.
    pub expired: String,
    /// Requested `(start, end)` per request.
    pub ranges: Arc<Mutex<Vec<(u64, u64)>>>,
}

pub fn data(n: usize) -> Arc<Vec<u8>> {
    Arc::new((0..n).map(|i| (i * 31 % 251) as u8).collect())
}

fn parse_range(head: &str) -> Option<(u64, u64)> {
    let path = head.lines().next()?.split_whitespace().nth(1)?;
    let from_param = path
        .split(['?', '&'])
        .find_map(|kv| kv.strip_prefix("range="))
        .map(str::to_owned);
    let from_header = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("range")
            .then(|| v.trim().trim_start_matches("bytes=").to_owned())
    });
    let r = from_param.or(from_header)?;
    let (a, b) = r.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

pub async fn serve(body: Arc<Vec<u8>>, mode: Mode) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let log = ranges.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let body = body.clone();
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let expired = head.starts_with("GET /expired");
                let len = body.len() as u64;
                let range = parse_range(&head);
                if let Some(r) = range.filter(|_| !expired) {
                    log.lock().unwrap().push(r);
                }
                let (a, b) = range.unwrap_or((0, len.saturating_sub(1)));
                let reply = |status: &str, extra: String, payload: &[u8]| {
                    let mut v = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                        payload.len()
                    )
                    .into_bytes();
                    v.extend_from_slice(payload);
                    v
                };
                let partial = |a: u64, b: u64| {
                    let b = b.min(len - 1);
                    reply(
                        "206 Partial Content",
                        format!("Content-Range: bytes {a}-{b}/{len}\r\n"),
                        &body[a as usize..=b as usize],
                    )
                };
                let out = match mode {
                    _ if expired => reply("403 Forbidden", String::new(), b""),
                    Mode::Forbidden => reply("403 Forbidden", String::new(), b""),
                    Mode::Whole => reply("200 OK", String::new(), &body),
                    _ if a >= len => reply("416 Range Not Satisfiable", String::new(), b""),
                    Mode::Param => reply(
                        "200 OK",
                        String::new(),
                        &body[a as usize..=(b.min(len - 1)) as usize],
                    ),
                    Mode::Partial => partial(a, b),
                    Mode::Slow(pause) => {
                        for piece in partial(a, b).chunks(4096) {
                            if sock.write_all(piece).await.is_err() {
                                return;
                            }
                            let _ = sock.flush().await;
                            tokio::time::sleep(pause).await;
                        }
                        let _ = sock.shutdown().await;
                        return;
                    }
                    Mode::Stall(n) => {
                        let piece = &body[a as usize..=(b.min(len - 1)) as usize];
                        let mut v = reply("200 OK", String::new(), piece);
                        v.truncate(v.len() - piece.len() + n);
                        let _ = sock.write_all(&v).await;
                        let _ = sock.flush().await;
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        return;
                    }
                };
                let _ = sock.write_all(&out).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Server {
        base: format!("http://{addr}/audio?clen_unknown=1"),
        expired: format!("http://{addr}/expired?clen_unknown=1"),
        ranges,
    }
}
