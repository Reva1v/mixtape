//! Cover URLs: YouTube Music hands out small thumbnails; these helpers ask for the
//! largest version and fall back to smaller ones when it does not exist.
//!
//! - `lh3.googleusercontent.com` / `yt3.ggpht.com`: the size lives in a suffix such as
//!   `=w120-h120-l90-rj` or `=s88-c-k`, and any size can be requested.
//! - `i.ytimg.com/vi/<id>/<name>.jpg` (video frames): only fixed names exist, and
//!   `maxresdefault`/`sddefault` are missing for many videos, so they are tried first and
//!   `hqdefault` is the safe fallback.

use crate::{Result, YtError};

/// Size options for a 1200 px square cover.
pub const HI_RES_OPTIONS: &str = "w1200-h1200-l90-rj";
/// Size options for a 300 px square card thumbnail.
pub const SMALL_OPTIONS: &str = "w300-h300-l90-rj";

const YTIMG_FALLBACK: &str = "hqdefault";
/// Names kept by [`hi_res`] because the API only returns them when they exist.
const YTIMG_LARGE: [&str; 3] = ["maxresdefault", "hq720", "sddefault"];

enum Shape<'a> {
    /// googleusercontent/ggpht: everything before the `=options` suffix.
    Google {
        base: &'a str,
    },
    /// i.ytimg.com frame thumbnail.
    Ytimg {
        id: &'a str,
        name: &'a str,
    },
    Other,
}

fn shape(url: &str) -> Shape<'_> {
    let Some((_, rest)) = url.split_once("://") else {
        return Shape::Other;
    };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    if host.ends_with("googleusercontent.com") || host.ends_with("ggpht.com") {
        let last_slash = url.rfind('/').unwrap_or(0);
        let base = match url.rfind('=') {
            Some(eq) if eq > last_slash => &url[..eq],
            _ => url,
        };
        return Shape::Google { base };
    }
    if host.ends_with("ytimg.com") || host == "img.youtube.com" {
        let path = path.split(['?', '#']).next().unwrap_or_default();
        let mut parts = path.split('/');
        if let (Some("vi" | "vi_webp"), Some(id), Some(file), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        {
            let name = file.rsplit_once('.').map_or(file, |(n, _)| n);
            if !id.is_empty() && !name.is_empty() {
                return Shape::Ytimg { id, name };
            }
        }
    }
    Shape::Other
}

/// `https:` for protocol-relative URLs (`//lh3…`), which the API sometimes returns.
fn absolute(url: &str) -> String {
    let url = url.trim();
    match url.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    }
}

fn ytimg(id: &str, name: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/{name}.jpg")
}

/// The best cover URL that is very likely to exist.
pub fn hi_res(url: &str) -> String {
    let url = absolute(url);
    match shape(&url) {
        Shape::Google { base } => format!("{base}={HI_RES_OPTIONS}"),
        Shape::Ytimg { name, .. } if YTIMG_LARGE.contains(&name) => url.clone(),
        Shape::Ytimg { id, .. } => ytimg(id, YTIMG_FALLBACK),
        Shape::Other => url.clone(),
    }
}

/// URLs to try in order, largest first; the original URL is always among them.
pub fn candidates(url: &str) -> Vec<String> {
    let url = absolute(url);
    let mut out = match shape(&url) {
        Shape::Google { base } => vec![format!("{base}={HI_RES_OPTIONS}")],
        Shape::Ytimg { id, name } => {
            let mut list = vec![ytimg(id, "maxresdefault")];
            // A signed `hq720` from the API exists for sure and beats `sddefault`.
            if name == "hq720" {
                list.push(url.clone());
            }
            list.push(ytimg(id, "sddefault"));
            list.push(ytimg(id, YTIMG_FALLBACK));
            list
        }
        Shape::Other => Vec::new(),
    };
    if !out.contains(&url) {
        out.push(url);
    }
    out
}

/// URLs to try for a card-sized (300 px) cover. Google images are asked for at that size;
/// video frames have no small letterbox-free name, so they go the [`candidates`] way.
pub fn small_candidates(url: &str) -> Vec<String> {
    let url = absolute(url);
    let mut out = match shape(&url) {
        Shape::Google { base } => vec![format!("{base}={SMALL_OPTIONS}")],
        Shape::Ytimg { .. } => return candidates(&url),
        Shape::Other => Vec::new(),
    };
    if !out.contains(&url) {
        out.push(url);
    }
    out
}

/// Downloads the largest available version of a cover (library items).
pub async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    fetch_first(client, &candidates(url)).await
}

/// Downloads a 300 px version of a cover (cards of things only browsed).
pub async fn download_small(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    fetch_first(client, &small_candidates(url)).await
}

/// The body of the first URL that answers 2xx with content.
async fn fetch_first(client: &reqwest::Client, urls: &[String]) -> Result<Vec<u8>> {
    let mut last = None;
    for url in urls {
        let resp = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                last = Some(YtError::from(e));
                continue;
            }
        };
        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(YtError::RateLimited);
        }
        if !status.is_success() {
            last = Some(YtError::Unavailable(format!(
                "Обложка недоступна (HTTP {})",
                status.as_u16()
            )));
            continue;
        }
        match resp.bytes().await {
            Ok(body) if !body.is_empty() => return Ok(body.to_vec()),
            Ok(_) => last = Some(YtError::Unavailable("Обложка пустая".into())),
            Err(e) => last = Some(YtError::from(e)),
        }
    }
    Err(last.unwrap_or_else(|| YtError::Unavailable("Нет ссылки на обложку".into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn google_sizes_are_replaced() {
        let cases = [
            (
                "https://lh3.googleusercontent.com/AbC_d-1=w120-h120-l90-rj",
                "https://lh3.googleusercontent.com/AbC_d-1",
            ),
            (
                "https://lh3.googleusercontent.com/AbC=w544-h544-s-l90-rj",
                "https://lh3.googleusercontent.com/AbC",
            ),
            (
                "https://yt3.ggpht.com/xyz=s88-c-k-c0x00ffffff-no-rj",
                "https://yt3.ggpht.com/xyz",
            ),
            (
                "https://yt3.googleusercontent.com/xyz=s120",
                "https://yt3.googleusercontent.com/xyz",
            ),
            (
                "https://lh3.googleusercontent.com/noSuffix",
                "https://lh3.googleusercontent.com/noSuffix",
            ),
            (
                "//lh3.googleusercontent.com/rel=w60-h60",
                "https://lh3.googleusercontent.com/rel",
            ),
        ];
        for (input, base) in cases {
            let want = format!("{base}=w1200-h1200-l90-rj");
            assert_eq!(hi_res(input), want, "{input}");
            let c = candidates(input);
            assert_eq!(c[0], want);
            assert_eq!(c.last().unwrap(), &absolute(input));
        }
        let small = small_candidates("https://lh3.googleusercontent.com/AbC=w120-h120-l90-rj");
        assert_eq!(
            small,
            [
                "https://lh3.googleusercontent.com/AbC=w300-h300-l90-rj",
                "https://lh3.googleusercontent.com/AbC=w120-h120-l90-rj"
            ]
        );
        // Already hi-res: no duplicate candidate.
        let hi = "https://lh3.googleusercontent.com/AbC=w1200-h1200-l90-rj";
        assert_eq!(candidates(hi), [hi]);
    }

    #[test]
    fn ytimg_frames_fall_back_to_fixed_names() {
        let small = "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg";
        assert_eq!(
            hi_res(small),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );
        assert_eq!(
            candidates(small),
            [
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg",
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/sddefault.jpg",
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg",
                small,
            ]
        );

        let webp = "https://i.ytimg.com/vi_webp/dQw4w9WgXcQ/default.webp";
        assert_eq!(
            hi_res(webp),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );

        let signed = "https://i.ytimg.com/vi/dQw4w9WgXcQ/hq720.jpg?sqp=-oaymwEXCNAF&rs=AOn4CLB";
        assert_eq!(hi_res(signed), signed);
        assert_eq!(candidates(signed)[1], signed);
        assert_eq!(candidates(signed).len(), 4);

        let maxres = "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg";
        assert_eq!(hi_res(maxres), maxres);
        assert_eq!(candidates(maxres).len(), 3);

        let hq = "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg?sqp=abc";
        assert_eq!(
            hi_res(hq),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );
        assert_eq!(candidates(hq).last().unwrap(), hq);
    }

    #[test]
    fn other_urls_are_left_alone() {
        for url in [
            "https://example.com/a.jpg",
            "not a url",
            "https://i.ytimg.com/an_webp/x/y/z.webp",
        ] {
            assert_eq!(hi_res(url), url);
            assert_eq!(candidates(url), [url]);
        }
    }

    /// Serves `/ok.jpg` (200) and `/limited.jpg` (429); everything else is 404.
    async fn serve() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/");
                    let (status, body) = match path {
                        "/ok.jpg" => ("200 OK", "JPEGDATA"),
                        "/limited.jpg" => ("429 Too Many Requests", ""),
                        _ => ("404 Not Found", "placeholder"),
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn download_tries_candidates_in_order() {
        let base = serve().await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let urls = [format!("{base}/maxres.jpg"), format!("{base}/ok.jpg")];
        assert_eq!(fetch_first(&client, &urls).await.unwrap(), b"JPEGDATA");

        let missing = [format!("{base}/a.jpg"), format!("{base}/b.jpg")];
        let err = fetch_first(&client, &missing).await.unwrap_err();
        assert!(
            matches!(err, YtError::Unavailable(ref s) if s.contains("404")),
            "{err:?}"
        );

        let limited = [format!("{base}/limited.jpg"), format!("{base}/ok.jpg")];
        assert!(matches!(
            fetch_first(&client, &limited).await,
            Err(YtError::RateLimited)
        ));
    }
}
