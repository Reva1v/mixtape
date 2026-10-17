//! YouTube cookies: import from a Netscape `cookies.txt` or an EditThisCookie JSON export,
//! the `Cookie` header, SAPISIDHASH request signing, and a Netscape copy for yt-dlp.
//!
//! Values never reach logs: `Debug` prints cookie names only.

use crate::{Result, YtError};
use mixtape_core::{tr, trf};
use sha1::{Digest, Sha1};
use std::fmt;

pub const ORIGIN: &str = "https://music.youtube.com";

#[derive(Clone, PartialEq, Eq)]
pub struct Cookie {
    pub domain: String,
    pub path: String,
    pub secure: bool,
    /// Unix seconds; 0 for session cookies.
    pub expires: u64,
    pub name: String,
    pub value: String,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Cookies {
    items: Vec<Cookie>,
}

impl fmt::Debug for Cookies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.items.iter().map(|c| &c.name)).finish()
    }
}

impl Cookies {
    /// Accepts either format; picks by the first non-blank character.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim_start_matches('\u{feff}').trim();
        let items = if text.starts_with('[') || text.starts_with('{') {
            parse_json(text)?
        } else {
            parse_netscape(text)
        };
        let items: Vec<Cookie> = items.into_iter().filter(|c| is_youtube(&c.domain)).collect();
        let cookies = Self { items };
        if cookies.sapisid().is_none() {
            return Err(YtError::Unavailable(
                tr(
                    "The file has no YouTube sign-in cookies (SAPISID), export them from the music.youtube.com page",
                    "У файлі немає cookies входу YouTube (SAPISID) — експортуйте їх зі сторінки music.youtube.com",
                    "в файле нет cookies входа YouTube (SAPISID) — экспортируйте их со страницы music.youtube.com",
                )
                .into(),
            ));
        }
        Ok(cookies)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn get(&self, name: &str) -> Option<&str> {
        // A `.youtube.com` cookie wins over a same-named one from another Google domain.
        let mut best: Option<&Cookie> = None;
        for c in self.items.iter().filter(|c| c.name == name) {
            if best.is_none() || c.domain.ends_with("youtube.com") {
                best = Some(c);
            }
        }
        best.map(|c| c.value.as_str())
    }

    fn sapisid(&self) -> Option<&str> {
        self.get("SAPISID").or_else(|| self.get("__Secure-3PAPISID"))
    }

    /// `Cookie:` header for requests to music.youtube.com.
    pub fn header(&self) -> String {
        let mut seen = std::collections::HashSet::new();
        let mut out = String::new();
        // Prefer youtube.com cookies, then the rest, never repeating a name.
        let ordered = self
            .items
            .iter()
            .filter(|c| c.domain.ends_with("youtube.com"))
            .chain(self.items.iter().filter(|c| !c.domain.ends_with("youtube.com")));
        for c in ordered {
            if seen.insert(c.name.as_str()) {
                if !out.is_empty() {
                    out.push_str("; ");
                }
                out.push_str(&c.name);
                out.push('=');
                out.push_str(&c.value);
            }
        }
        out
    }

    /// `Authorization:` value as music.youtube.com sends it at unix time `now`.
    pub fn authorization(&self, now: u64) -> Option<String> {
        let sapisid = self.sapisid()?;
        let hash = |value: &str| {
            let digest = Sha1::digest(format!("{now} {value} {ORIGIN}").as_bytes());
            format!("{now}_{}", hex(&digest))
        };
        let mut out = format!("SAPISIDHASH {}", hash(sapisid));
        if let Some(v) = self.get("__Secure-1PAPISID") {
            out.push_str(&format!(" SAPISID1PHASH {}", hash(v)));
        }
        if let Some(v) = self.get("__Secure-3PAPISID") {
            out.push_str(&format!(" SAPISID3PHASH {}", hash(v)));
        }
        Some(out)
    }

    /// Netscape `cookies.txt` text, the format yt-dlp reads and the one we store.
    pub fn to_netscape(&self) -> String {
        let mut out = String::from("# Netscape HTTP Cookie File\n");
        for c in &self.items {
            let sub = if c.domain.starts_with('.') { "TRUE" } else { "FALSE" };
            let secure = if c.secure { "TRUE" } else { "FALSE" };
            out.push_str(&format!(
                "{}\t{sub}\t{}\t{secure}\t{}\t{}\t{}\n",
                c.domain, c.path, c.expires, c.name, c.value
            ));
        }
        out
    }
}

fn is_youtube(domain: &str) -> bool {
    let d = domain.trim_start_matches('.');
    d == "youtube.com" || d.ends_with(".youtube.com") || d == "google.com" || d.ends_with(".google.com")
}

fn parse_netscape(text: &str) -> Vec<Cookie> {
    text.lines()
        .filter_map(|line| {
            // curl marks HttpOnly cookies with this prefix instead of a comment.
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') || line.trim().is_empty() {
                return None;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                return None;
            }
            Some(Cookie {
                domain: f[0].to_owned(),
                path: f[2].to_owned(),
                secure: f[3].eq_ignore_ascii_case("TRUE"),
                expires: f[4].trim().parse::<f64>().map(|v| v as u64).unwrap_or(0),
                name: f[5].to_owned(),
                value: f[6].trim_end_matches('\r').to_owned(),
            })
        })
        .collect()
}

/// EditThisCookie / Cookie-Editor export: `[{domain, path, secure, expirationDate, name, value}]`.
fn parse_json(text: &str) -> Result<Vec<Cookie>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| {
            YtError::Unavailable(trf!(
                "Couldn't read the cookies JSON: {e}",
                "Не вдалося прочитати JSON cookies: {e}",
                "не получилось прочитать JSON cookies: {e}",
            ))
        })?;
    let list = match &value {
        serde_json::Value::Array(a) => a.as_slice(),
        serde_json::Value::Object(o) => o.get("cookies").and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[]),
        _ => &[],
    };
    Ok(list
        .iter()
        .filter_map(|c| {
            let s = |k: &str| c.get(k).and_then(|v| v.as_str()).map(str::to_owned);
            Some(Cookie {
                domain: s("domain")?,
                path: s("path").unwrap_or_else(|| "/".into()),
                secure: c.get("secure").and_then(|v| v.as_bool()).unwrap_or(false),
                expires: c.get("expirationDate").and_then(|v| v.as_f64()).map(|v| v as u64).unwrap_or(0),
                name: s("name")?,
                value: s("value")?,
            })
        })
        .collect())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETSCAPE: &str = "# Netscape HTTP Cookie File\n\
        .youtube.com\tTRUE\t/\tTRUE\t1826751447\t__Secure-1PAPISID\tone\n\
        .youtube.com\tTRUE\t/\tTRUE\t1826751447\t__Secure-3PAPISID\tthree\n\
        .youtube.com\tTRUE\t/\tFALSE\t1826751447\tSAPISID\tplain\n\
        #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1826751447\tSID\tsid\n\
        .example.com\tTRUE\t/\tFALSE\t0\tOTHER\tx\n";

    const JSON: &str = r#"[
        {"domain": ".youtube.com", "path": "/", "secure": true, "expirationDate": 1826751447.5, "name": "__Secure-3PAPISID", "value": "three"},
        {"domain": ".youtube.com", "path": "/", "secure": false, "expirationDate": 1826751447, "name": "SAPISID", "value": "plain"},
        {"domain": ".google.com", "path": "/", "secure": false, "name": "SAPISID", "value": "google"},
        {"domain": "music.youtube.com", "path": "/", "secure": false, "session": true, "name": "PREF", "value": "hl=ru"}
    ]"#;

    #[test]
    fn netscape_keeps_youtube_cookies_and_httponly_lines() {
        let c = Cookies::parse(NETSCAPE).unwrap();
        assert_eq!(c.len(), 4);
        assert_eq!(c.header(), "__Secure-1PAPISID=one; __Secure-3PAPISID=three; SAPISID=plain; SID=sid");
    }

    #[test]
    fn json_export_prefers_youtube_domain() {
        let c = Cookies::parse(JSON).unwrap();
        assert_eq!(c.get("SAPISID"), Some("plain"));
        assert!(c.header().contains("PREF=hl=ru"));
        assert!(!c.header().contains("google"));
    }

    #[test]
    fn signature_matches_the_web_client() {
        let c = Cookies::parse(NETSCAPE).unwrap();
        let auth = c.authorization(1_700_000_000).unwrap();
        let expect = |v: &str| {
            let d = Sha1::digest(format!("1700000000 {v} {ORIGIN}").as_bytes());
            format!("1700000000_{}", hex(&d))
        };
        assert_eq!(
            auth,
            format!(
                "SAPISIDHASH {} SAPISID1PHASH {} SAPISID3PHASH {}",
                expect("plain"),
                expect("one"),
                expect("three")
            )
        );
    }

    #[test]
    fn netscape_round_trip() {
        let c = Cookies::parse(JSON).unwrap();
        assert_eq!(Cookies::parse(&c.to_netscape()).unwrap(), c);
    }

    #[test]
    fn rejects_files_without_login() {
        assert!(Cookies::parse(".youtube.com\tTRUE\t/\tFALSE\t0\tPREF\tx").is_err());
    }

    #[test]
    fn debug_hides_values() {
        let c = Cookies::parse(NETSCAPE).unwrap();
        assert!(!format!("{c:?}").contains("plain"));
    }
}
