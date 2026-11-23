//! Notice when a new stable yt-dlp is out. Nothing is downloaded or installed: the bundled
//! yt-dlp reports its own version, the public GitHub API reports the latest release, and
//! the app only compares the two.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::innertube::{BodyError, read_limited};
use crate::resolver::{run_capture, ytdlp_command};
use crate::{Result, YtError};

const LATEST_URL: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY: usize = 512 * 1024;

/// `2026.10.26` or `2026.11.25.1` as numbers; `None` for anything else (a `v` prefix,
/// nightly suffixes, empty text).
pub fn parse_version(s: &str) -> Option<Vec<u32>> {
    let parts: Vec<u32> = s
        .trim()
        .split('.')
        .map(|p| {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        })
        .collect::<Option<_>>()?;
    (3..=4).contains(&parts.len()).then_some(parts)
}

/// `latest` is a newer version than `installed`; false when either does not parse.
pub fn is_newer(latest: &str, installed: &str) -> bool {
    match (parse_version(latest), parse_version(installed)) {
        (Some(mut l), Some(mut i)) => {
            let n = l.len().max(i.len());
            l.resize(n, 0);
            i.resize(n, 0);
            l > i
        }
        _ => false,
    }
}

/// `tag_name` of a GitHub release answer; none for a draft, a prerelease or a broken answer.
pub fn latest_tag(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    if flag("draft") || flag("prerelease") {
        return None;
    }
    let tag = v.get("tag_name")?.as_str()?.trim();
    (!tag.is_empty()).then(|| tag.to_owned())
}

/// A release asset as GitHub lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A stable GitHub release: its tag, when it was published (Unix seconds) and its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub published_at: i64,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// `2026-11-20T12:34:56Z` as Unix seconds; `None` for any other shape.
pub fn parse_utc_time(s: &str) -> Option<i64> {
    let b = s.trim().as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = &b[r];
        if !t.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(t).ok()?.parse().ok()
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3_600 + mi * 60 + sec)
}

/// The stable release in a GitHub answer; none for a draft, a prerelease, a missing or
/// unreadable tag or date, or a broken answer.
pub fn parse_release(json: &str) -> Option<Release> {
    let tag = latest_tag(json)?;
    let v: Value = serde_json::from_str(json).ok()?;
    let published_at = parse_utc_time(v.get("published_at")?.as_str()?)?;
    let assets = v
        .get("assets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a.get("name")?.as_str()?.to_owned(),
                        url: a.get("browser_download_url")?.as_str()?.to_owned(),
                        size: a.get("size")?.as_u64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Release { tag, published_at, assets })
}

/// The latest stable yt-dlp release from GitHub. The only network call of the notice.
pub async fn fetch_release(http: &reqwest::Client) -> Result<Release> {
    let resp = http
        .get(LATEST_URL)
        .header(reqwest::header::USER_AGENT, "Mixtape")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(FETCH_TIMEOUT)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(YtError::Network(format!("GitHub answered {status}")));
    }
    let body = read_limited(resp, MAX_BODY).await.map_err(|e| match e {
        BodyError::Http(e) => YtError::from(e),
        e @ BodyError::TooLarge(_) => YtError::Parse(format!("yt-dlp release: {e}")),
    })?;
    parse_release(&String::from_utf8_lossy(&body))
        .ok_or_else(|| YtError::Parse("yt-dlp release: no stable release".into()))
}

/// The latest stable yt-dlp release tag.
pub async fn fetch_latest(http: &reqwest::Client) -> Result<String> {
    Ok(fetch_release(http).await?.tag)
}

/// What `yt-dlp --version` of the bundled executable prints (first line).
pub async fn installed_version(ytdlp: &Path) -> Result<String> {
    let work_dir = std::env::temp_dir();
    let mut cmd = ytdlp_command(ytdlp, &work_dir);
    cmd.arg("--version");
    let (out, _) = run_capture(cmd, VERSION_TIMEOUT)
        .await
        .map_err(YtError::Tool)?;
    let line = out.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return Err(YtError::Tool("--version printed nothing".into()));
    }
    Ok(line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::path::PathBuf;

    #[test]
    fn parses_three_and_four_parts() {
        assert_eq!(parse_version("2026.10.26"), Some(vec![2026, 10, 26]));
        assert_eq!(
            parse_version(" 2026.11.25.1\n"),
            Some(vec![2026, 11, 25, 1])
        );
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "  ",
            "v2026.10.26",
            "2026.10",
            "2026.10.26.1.2",
            "2026.10.x",
            "2026..26",
            "2026.10.26-nightly",
            "+1.2.3",
            "-1.2.3",
            "99999999999.1.1",
        ] {
            assert_eq!(parse_version(s), None, "{s:?}");
        }
    }

    #[test]
    fn compares_numerically() {
        assert!(!is_newer("2026.10.26", "2026.10.26"));
        assert!(!is_newer("2026.10.26", "2026.11.25"));
        assert!(is_newer("2026.11.25", "2026.10.26"));
        assert!(is_newer("2026.11.25", "2026.11.9"));
        assert!(is_newer("2026.11.25.1", "2026.11.25"));
        assert!(!is_newer("2026.11.25", "2026.11.25.1"));
        assert!(!is_newer("2026.11.25", "2026.11.25.0"));
        assert!(!is_newer("junk", "2026.10.26"));
        assert!(!is_newer("2026.11.25", "junk"));
        assert!(!is_newer("", ""));
    }

    #[test]
    fn reads_the_tag() {
        assert_eq!(
            latest_tag(r#"{"tag_name":"2026.11.25","draft":false,"prerelease":false}"#),
            Some("2026.11.25".into())
        );
        assert_eq!(
            latest_tag(r#"{"tag_name":"2026.11.25"}"#),
            Some("2026.11.25".into())
        );
        assert_eq!(
            latest_tag(r#"{"tag_name":"2026.11.25","prerelease":true}"#),
            None
        );
        assert_eq!(
            latest_tag(r#"{"tag_name":"2026.11.25","draft":true}"#),
            None
        );
        assert_eq!(latest_tag(r#"{"name":"x"}"#), None);
        assert_eq!(latest_tag(r#"{"tag_name":""}"#), None);
        assert_eq!(latest_tag(r#"{"tag_name":5}"#), None);
        assert_eq!(latest_tag("not json"), None);
        assert_eq!(latest_tag(""), None);
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_utc_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc_time("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_utc_time("2026-11-24T12:00:00Z"), Some(1_795_521_600));
        for s in ["", "2026-11-24", "2026-13-24T00:00:00Z", "2026-11-24T25:00:00Z", "2026-11-24T00:00:00+01:00", "2026-11-2xT00:00:00Z"] {
            assert_eq!(parse_utc_time(s), None, "{s:?}");
        }
    }

    #[test]
    fn reads_a_release() {
        let j = r#"{"tag_name":"2026.11.25","published_at":"2026-11-25T10:00:00Z","assets":[
            {"name":"yt-dlp.exe","browser_download_url":"https://github.com/yt-dlp/yt-dlp/releases/download/2026.11.25/yt-dlp.exe","size":123},
            {"name":"broken"}]}"#;
        let r = parse_release(j).unwrap();
        assert_eq!(r.tag, "2026.11.25");
        assert_eq!(r.assets.len(), 1);
        assert_eq!(r.asset("yt-dlp.exe").unwrap().size, 123);
        assert!(r.asset("SHA2-256SUMS").is_none());
        assert!(parse_release(r#"{"tag_name":"2026.11.25","published_at":"nope"}"#).is_none());
        assert!(parse_release(r#"{"tag_name":"2026.11.25"}"#).is_none());
        assert!(parse_release(r#"{"tag_name":"2026.11.25","published_at":"2026-11-25T10:00:00Z","draft":true}"#).is_none());
    }

    #[cfg(windows)]
    fn fake(body: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mixtape-yt-rel-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("yt-dlp.cmd");
        std::fs::write(&script, format!("@echo off\r\n{body}\r\n")).unwrap();
        (dir, script)
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn reads_installed_version_from_the_executable() {
        let (dir, script) = fake("echo 2026.10.26");
        assert_eq!(installed_version(&script).await.unwrap(), "2026.10.26");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn failing_or_missing_executable_is_an_error() {
        let (dir, script) = fake("echo boom 1>&2\r\nexit /b 1");
        assert!(installed_version(&script).await.is_err());
        let _ = std::fs::remove_dir_all(dir);
        assert!(
            installed_version(Path::new("Z:\\no\\such\\yt-dlp.exe"))
                .await
                .is_err()
        );
    }
}
