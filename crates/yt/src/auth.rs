//! YouTube cookies: import from a Netscape `cookies.txt` or an EditThisCookie JSON export,
//! the `Cookie` header, SAPISIDHASH request signing, and a Netscape copy for yt-dlp.
//!
//! Values never reach logs: `Debug` prints cookie names only.

use crate::{Result, YtError};
use mixtape_core::{tr, trf};
use sha1::{Digest, Sha1};
use std::fmt;

pub const ORIGIN: &str = "https://music.youtube.com";

/// The cookies Google keeps replacing on a live session; the rest of a jar lasts for years.
const ROTATING: [&str; 5] = [
    "SIDCC",
    "__Secure-1PSIDCC",
    "__Secure-3PSIDCC",
    "__Secure-1PSIDTS",
    "__Secure-3PSIDTS",
];

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
        f.debug_list()
            .entries(self.items.iter().map(|c| &c.name))
            .finish()
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
        let items: Vec<Cookie> = items
            .into_iter()
            .filter(|c| is_youtube(&c.domain))
            .collect();
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

    /// Cookies from a `Cookie:` header value (`a=1; b=2`), as a web view hands them out for
    /// music.youtube.com: names and values only, so they are filed under `.youtube.com`.
    pub fn from_header(header: &str) -> Result<Self> {
        let mut text = String::from("# Netscape HTTP Cookie File\n");
        for pair in header.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if name.is_empty() || name.contains(['\t', '\n']) || value.contains(['\t', '\n']) {
                continue;
            }
            text.push_str(&format!(
                ".youtube.com\tTRUE\t/\tTRUE\t0\t{name}\t{value}\n"
            ));
        }
        Self::parse(&text)
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
        self.get("SAPISID")
            .or_else(|| self.get("__Secure-3PAPISID"))
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
            .chain(
                self.items
                    .iter()
                    .filter(|c| !c.domain.ends_with("youtube.com")),
            );
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

    /// Applies `Set-Cookie` values from an answer at unix time `now`; true if a value changed.
    /// Nothing is ever deleted: Google clears cookies when it invalidates a session, and the
    /// jar is what shows the session was signed in. Empty and already expired values are
    /// skipped for that reason.
    pub fn merge_set_cookies<'a>(
        &mut self,
        headers: impl IntoIterator<Item = &'a str>,
        default_domain: &str,
        now: u64,
    ) -> bool {
        let mut changed = false;
        for header in headers {
            if let Some(cookie) = parse_set_cookie(header, default_domain, now) {
                changed |= self.upsert(cookie);
            }
        }
        changed
    }

    /// Both jars hold one sign-in: its key stays while the rotating cookies change.
    pub fn same_session(&self, other: &Cookies) -> bool {
        self.sapisid() == other.sapisid()
    }

    /// Takes the session's rotating cookies from a copy of this jar (the one yt-dlp saved);
    /// true if a value changed. A jar of another session is ignored.
    pub fn merge_rotating(&mut self, from: &Cookies) -> bool {
        if self.sapisid() != from.sapisid() {
            return false;
        }
        let mut changed = false;
        for c in &from.items {
            if ROTATING.contains(&c.name.as_str()) && !c.value.is_empty() {
                changed |= self.upsert(c.clone());
            }
        }
        changed
    }

    fn upsert(&mut self, new: Cookie) -> bool {
        let same = |c: &&mut Cookie| {
            c.name == new.name
                && c.domain.trim_start_matches('.') == new.domain.trim_start_matches('.')
        };
        match self.items.iter_mut().find(same) {
            Some(c) => {
                let changed = c.value != new.value;
                c.value = new.value;
                if new.expires != 0 {
                    c.expires = new.expires;
                }
                changed
            }
            None => {
                self.items.push(new);
                true
            }
        }
    }

    /// Netscape `cookies.txt` text, the format yt-dlp reads and the one we store.
    pub fn to_netscape(&self) -> String {
        let mut out = String::from("# Netscape HTTP Cookie File\n");
        for c in &self.items {
            let sub = if c.domain.starts_with('.') {
                "TRUE"
            } else {
                "FALSE"
            };
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
    d == "youtube.com"
        || d.ends_with(".youtube.com")
        || d == "google.com"
        || d.ends_with(".google.com")
}

/// One `Set-Cookie` value, or `None` if it clears a cookie or is not for YouTube or Google.
fn parse_set_cookie(header: &str, default_domain: &str, now: u64) -> Option<Cookie> {
    let mut parts = header.split(';');
    let (name, value) = parts.next()?.trim().split_once('=')?;
    let bad = |s: &str| s.contains(['\t', '\n', '\r']);
    if name.is_empty() || value.is_empty() || bad(name) || bad(value) {
        return None;
    }
    let mut cookie = Cookie {
        domain: default_domain.to_owned(),
        path: "/".to_owned(),
        secure: false,
        expires: 0,
        name: name.to_owned(),
        value: value.to_owned(),
    };
    let (mut max_age, mut expires) = (None, None);
    for attr in parts {
        let (key, val) = attr.trim().split_once('=').unwrap_or((attr.trim(), ""));
        match key.to_ascii_lowercase().as_str() {
            "domain" if !val.is_empty() && !bad(val) => {
                cookie.domain = format!(".{}", val.trim_start_matches('.'));
            }
            "path" if !val.is_empty() && !bad(val) => cookie.path = val.to_owned(),
            "secure" => cookie.secure = true,
            "max-age" => max_age = val.parse::<i64>().ok(),
            "expires" => expires = parse_http_date(val),
            _ => {}
        }
    }
    // Max-Age wins over Expires, as in a browser.
    cookie.expires = match (max_age, expires) {
        (Some(age), _) if age <= 0 => return None,
        (Some(age), _) => now.saturating_add(age as u64),
        (None, Some(at)) if at <= now => return None,
        (None, Some(at)) => at,
        (None, None) => 0,
    };
    is_youtube(&cookie.domain).then_some(cookie)
}

/// Unix seconds of `Tue, 02 Nov 2027 21:10:00 GMT` or `Tue, 02-Nov-2027 21:10:00 GMT`.
fn parse_http_date(text: &str) -> Option<u64> {
    let (_, rest) = text.split_once(',')?;
    let fields: Vec<&str> = rest.split([' ', '-']).filter(|s| !s.is_empty()).collect();
    let [day, month, year, clock, ..] = fields.as_slice() else {
        return None;
    };
    let day: i64 = day.parse().ok()?;
    let year: i64 = year.parse().ok()?;
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let month = MONTHS.iter().position(|m| month.eq_ignore_ascii_case(m))? as i64 + 1;
    let mut clock = clock.split(':').map(|v| v.parse::<i64>().ok());
    let (h, m, s) = (clock.next()??, clock.next()??, clock.next()??);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + m * 60 + s).ok()
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
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| {
        YtError::Unavailable(trf!(
            "Couldn't read the cookies JSON: {e}",
            "Не вдалося прочитати JSON cookies: {e}",
            "не получилось прочитать JSON cookies: {e}",
        ))
    })?;
    let list = match &value {
        serde_json::Value::Array(a) => a.as_slice(),
        serde_json::Value::Object(o) => o
            .get("cookies")
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[]),
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
                expires: c
                    .get("expirationDate")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as u64)
                    .unwrap_or(0),
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

    #[test]
    fn reads_a_web_view_cookie_header() {
        let c = Cookies::from_header(
            "VISITOR_INFO1_LIVE=v; SAPISID=plain; __Secure-3PAPISID=three; SID=s=id;bad",
        )
        .unwrap();
        assert_eq!(c.len(), 4);
        assert_eq!(c.sapisid(), Some("plain"));
        // A value may itself hold `=`.
        assert!(c.header().contains("SID=s=id"));
        assert!(
            c.to_netscape()
                .contains(".youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tplain")
        );
        assert!(
            Cookies::from_header("PREF=f6=40000000; YSC=x").is_err(),
            "no sign-in cookie"
        );
    }

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
        assert_eq!(
            c.header(),
            "__Secure-1PAPISID=one; __Secure-3PAPISID=three; SAPISID=plain; SID=sid"
        );
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

    const NOW: u64 = 1_800_000_000;

    fn value(c: &Cookies, name: &str) -> Option<String> {
        c.get(name).map(str::to_owned)
    }

    #[test]
    fn set_cookie_replaces_a_value_and_keeps_the_rest() {
        let mut c = Cookies::parse(NETSCAPE).unwrap();
        let changed = c.merge_set_cookies(
            ["SID=new; Domain=.youtube.com; Path=/; Secure; HttpOnly; Max-Age=3600"],
            "music.youtube.com",
            NOW,
        );
        assert!(changed);
        assert_eq!(value(&c, "SID").as_deref(), Some("new"));
        assert_eq!(value(&c, "SAPISID").as_deref(), Some("plain"));
        assert_eq!(c.len(), 4, "the same cookie, not a second one");
        assert!(
            c.to_netscape()
                .contains(&format!("{}\tSID\tnew", NOW + 3600))
        );
        assert_eq!(Cookies::parse(&c.to_netscape()).unwrap(), c);
    }

    #[test]
    fn set_cookie_adds_a_new_cookie() {
        let mut c = Cookies::parse(NETSCAPE).unwrap();
        assert!(c.merge_set_cookies(
            ["__Secure-1PSIDTS=ts; Domain=.youtube.com; Expires=Tue, 02-Nov-2027 21:10:00 GMT"],
            "accounts.youtube.com",
            NOW,
        ));
        assert_eq!(value(&c, "__Secure-1PSIDTS").as_deref(), Some("ts"));
        assert!(
            c.to_netscape().contains("1825189800\t__Secure-1PSIDTS\tts"),
            "{:?}",
            c.to_netscape().lines().last().map(|l| l.split('\t').nth(4))
        );
    }

    #[test]
    fn set_cookie_never_clears_the_session() {
        let mut c = Cookies::parse(NETSCAPE).unwrap();
        let before = c.clone();
        let changed = c.merge_set_cookies(
            [
                // How Google signs a session out.
                "SID=; Domain=.youtube.com; Max-Age=0",
                "SAPISID=gone; Domain=.youtube.com; Max-Age=0",
                "SID=gone; Domain=.youtube.com; Expires=Thu, 01-Jan-1970 00:00:01 GMT",
                // Another site, and values that would break the Netscape file.
                "SID=evil; Domain=.example.com",
                "SID=a\tb; Domain=.youtube.com",
                "no equals sign",
            ],
            "music.youtube.com",
            NOW,
        );
        assert!(!changed);
        assert_eq!(c, before);
    }

    #[test]
    fn same_value_is_no_change() {
        let mut c = Cookies::parse(NETSCAPE).unwrap();
        assert!(!c.merge_set_cookies(["SID=sid; Domain=.youtube.com"], "music.youtube.com", NOW));
    }

    #[test]
    fn dates_of_both_shapes_parse() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:01 GMT"), Some(1));
        assert_eq!(
            parse_http_date("Tue, 02-Nov-2027 21:10:00 GMT"),
            Some(1_825_189_800)
        );
        assert_eq!(
            parse_http_date("Sat, 29 Feb 2020 12:00:00 GMT"),
            Some(1_582_977_600)
        );
        assert_eq!(parse_http_date("soon"), None);
    }

    #[test]
    fn only_rotating_cookies_come_back_from_a_copy() {
        let mut c = Cookies::parse(NETSCAPE).unwrap();
        let copy = Cookies::parse(
            ".youtube.com\tTRUE\t/\tFALSE\t0\tSAPISID\tplain\n\
             .youtube.com\tTRUE\t/\tTRUE\t0\tSID\trewritten\n\
             .youtube.com\tTRUE\t/\tTRUE\t0\t__Secure-1PSIDTS\trotated\n\
             .youtube.com\tTRUE\t/\tTRUE\t0\tPREF\trewritten\n",
        )
        .unwrap();
        assert!(c.merge_rotating(&copy));
        assert_eq!(value(&c, "__Secure-1PSIDTS").as_deref(), Some("rotated"));
        assert_eq!(value(&c, "SID").as_deref(), Some("sid"));
        assert_eq!(value(&c, "PREF"), None);
    }

    #[test]
    fn a_refreshed_jar_is_the_same_session_and_a_new_sign_in_is_not() {
        let jar = Cookies::parse(NETSCAPE).unwrap();
        let mut refreshed = jar.clone();
        refreshed.merge_set_cookies(["SIDCC=new; Domain=.youtube.com"], "music.youtube.com", NOW);
        assert!(jar.same_session(&refreshed));
        let other = Cookies::parse(".youtube.com\tTRUE\t/\tFALSE\t0\tSAPISID\tother\n").unwrap();
        assert!(!jar.same_session(&other));
    }

    #[test]
    fn a_copy_of_another_session_is_ignored() {
        let mut c = Cookies::parse(NETSCAPE).unwrap();
        let other = Cookies::parse(
            ".youtube.com\tTRUE\t/\tFALSE\t0\tSAPISID\tother\n\
             .youtube.com\tTRUE\t/\tTRUE\t0\t__Secure-1PSIDTS\tforeign\n",
        )
        .unwrap();
        assert!(!c.merge_rotating(&other));
        assert_eq!(value(&c, "__Secure-1PSIDTS"), None);
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
