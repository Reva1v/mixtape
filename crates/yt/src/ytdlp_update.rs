//! Delayed, verified auto-update of yt-dlp (spec 003, auto-update addendum).
//!
//! A new stable release is installed only when it is at least [`MIN_AGE_SECS`] old, its
//! `SHA2-256SUMS` carries a valid signature of yt-dlp's key (embedded in the app, checked
//! in pure Rust), the downloaded executable matches the signed hash, and a smoke test of the
//! staged executable passes (that part is the caller's, it needs the resolver). The updated
//! copy lives in a tools folder of its own, never next to the installed app.
//!
//! Everything here is either pure or touches only the folders it is given.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use pgp::composed::{Deserializable, DetachedSignature, SignedPublicKey};
use pgp::types::KeyDetails as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::innertube::BodyError;
use crate::ytdlp_release::{Asset, Release, is_newer, parse_version};

/// A release must be this old before it installs (5 days).
pub const MIN_AGE_SECS: i64 = 5 * 24 * 60 * 60;
pub const EXE_ASSET: &str = "yt-dlp.exe";
pub const SUMS_ASSET: &str = "SHA2-256SUMS";
pub const SIG_ASSET: &str = "SHA2-256SUMS.sig";
pub const MAX_EXE: u64 = 64 * 1024 * 1024;
pub const MAX_SUMS: u64 = 64 * 1024;
pub const MAX_SIG: u64 = 8 * 1024;
/// Failures in a row of the new exe (with the previous one working) that trigger a rollback.
pub const ROLLBACK_STRIKES: u32 = 3;

/// yt-dlp's public signing key, as published in the yt-dlp repository
/// (`public.key`, commit 12647e03d417feaa9ea6a458bea5ebd747494a53).
pub const PUBLIC_KEY: &str = include_str!("../keys/yt-dlp-public.key");
/// Fingerprint of [`PUBLIC_KEY`] (yt-dlp signing key, Simon Sawicki).
pub const PUBLIC_KEY_FINGERPRINT: &str = "AC0CBBE6848D6A873464AF4E57CF65933B5A7581";

const HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// Why a candidate was refused or an update step failed; goes to the log.
pub type Refusal = String;

// ---- delay -----------------------------------------------------------------

/// The release is old enough to install (a date in the future never is).
pub fn old_enough(published_at: i64, now: i64) -> bool {
    now.saturating_sub(published_at) >= MIN_AGE_SECS
}

/// When a release published at `published_at` becomes installable.
pub fn install_after(published_at: i64) -> i64 {
    published_at.saturating_add(MIN_AGE_SECS)
}

// ---- assets ----------------------------------------------------------------

/// An asset URL we may download from: https and one of GitHub's own hosts (exactly).
pub fn allowed_url(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && u.port().is_none()
        && u.host_str().is_some_and(|h| HOSTS.contains(&h))
}

/// The asset `name` of the release if its URL is allowed and its announced size fits `cap`.
pub fn pick_asset<'a>(rel: &'a Release, name: &str, cap: u64) -> Result<&'a Asset, Refusal> {
    let a = rel
        .asset(name)
        .ok_or_else(|| format!("release {} has no {name}", rel.tag))?;
    if !allowed_url(&a.url) {
        return Err(format!("{name}: URL not allowed"));
    }
    if a.size == 0 || a.size > cap {
        return Err(format!("{name}: size {} outside 1..={cap}", a.size));
    }
    Ok(a)
}

// ---- hashes and signature --------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// SHA-256 of a file, read in chunks.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// The lowercase hex SHA-256 listed for `file` in a `SHA2-256SUMS` text. The file must
/// appear exactly once and its hash must be 64 hex digits.
pub fn parse_sums(text: &str, file: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let Some((h, name)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        if name.trim_start().trim_start_matches('*') != file {
            continue;
        }
        if found.is_some() || h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        found = Some(h.to_ascii_lowercase());
    }
    found
}

fn load_key(armored: &str) -> Result<SignedPublicKey, Refusal> {
    SignedPublicKey::from_string(armored)
        .map(|(k, _)| k)
        .map_err(|e| format!("public key: {e}"))
}

/// Upper-case hex fingerprint of an armored public key.
pub fn key_fingerprint(armored: &str) -> Result<String, Refusal> {
    let key = load_key(armored)?;
    Ok(format!("{:X}", key.fingerprint()))
}

/// `sig` (a binary detached OpenPGP signature) is a valid signature of `data` by the key.
pub fn verify_signature(armored_key: &str, data: &[u8], sig: &[u8]) -> Result<(), Refusal> {
    let key = load_key(armored_key)?;
    let sig = DetachedSignature::from_bytes(sig).map_err(|e| format!("signature: {e}"))?;
    sig.verify(&key, data)
        .map_err(|e| format!("signature does not match: {e}"))
}

/// What has to hold before a downloaded exe may even be smoke-tested: the sums are signed by
/// `armored_key` and list the exe's hash. Returns that hash.
pub fn verify_download(
    armored_key: &str,
    sums: &[u8],
    sig: &[u8],
    exe_sha256: &str,
) -> Result<String, Refusal> {
    verify_signature(armored_key, sums, sig)?;
    let text = std::str::from_utf8(sums).map_err(|_| "sums are not UTF-8".to_owned())?;
    let want = parse_sums(text, EXE_ASSET)
        .ok_or_else(|| format!("sums have no single valid {EXE_ASSET} line"))?;
    if !want.eq_ignore_ascii_case(exe_sha256) {
        return Err(format!(
            "{EXE_ASSET} hash {exe_sha256} differs from the signed {want}"
        ));
    }
    Ok(want)
}

// ---- download --------------------------------------------------------------

/// A client that follows redirects only to allowed hosts.
pub fn download_client() -> Result<reqwest::Client, Refusal> {
    let policy = reqwest::redirect::Policy::custom(|a| {
        if a.previous().len() >= 5 {
            a.error("too many redirects")
        } else if allowed_url(a.url().as_str()) {
            a.follow()
        } else {
            a.error("redirect to a host that is not allowed")
        }
    });
    reqwest::Client::builder()
        .user_agent("Mixtape")
        .redirect(policy)
        .build()
        .map_err(|e| format!("http client: {e}"))
}

/// Downloads `asset` into `dest` (parent must exist), refusing more than `cap` bytes and
/// any final URL that is not allowed.
pub async fn download(
    http: &reqwest::Client,
    asset: &Asset,
    cap: u64,
    dest: &Path,
) -> Result<(), Refusal> {
    if !allowed_url(&asset.url) {
        return Err(format!("{}: URL not allowed", asset.name));
    }
    let resp = http
        .get(&asset.url)
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| format!("{}: {e}", asset.name))?;
    if !resp.status().is_success() {
        return Err(format!("{}: HTTP {}", asset.name, resp.status()));
    }
    if !allowed_url(resp.url().as_str()) {
        return Err(format!("{}: final URL not allowed", asset.name));
    }
    let mut resp = resp;
    if resp.content_length().is_some_and(|n| n > cap) {
        return Err(format!(
            "{}: {}",
            asset.name,
            BodyError::TooLarge(cap as usize)
        ));
    }
    let mut file = std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut total = 0u64;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("{}: {e}", asset.name))?
    {
        total += chunk.len() as u64;
        if total > cap {
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(format!(
                "{}: {}",
                asset.name,
                BodyError::TooLarge(cap as usize)
            ));
        }
        file.write_all(&chunk)
            .map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    Ok(())
}

// ---- tools folder ----------------------------------------------------------

/// `tools\yt-dlp.json`: what the updated copy is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsMeta {
    pub version: String,
    pub sha256: String,
    pub installed_at: i64,
    /// The version that was replaced (kept in `previous\`), if any.
    #[serde(default)]
    pub previous: Option<String>,
}

pub fn tools_exe(dir: &Path) -> PathBuf {
    dir.join(EXE_ASSET)
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("yt-dlp.json")
}

fn previous_dir(dir: &Path) -> PathBuf {
    dir.join("previous")
}

/// The previous copy kept for a rollback, if it is there.
pub fn previous_exe(dir: &Path) -> Option<PathBuf> {
    let exe = tools_exe(&previous_dir(dir));
    exe.exists().then_some(exe)
}

pub fn staging_dir(dir: &Path) -> PathBuf {
    dir.join("staging")
}

fn bad_path(dir: &Path) -> PathBuf {
    dir.join("bad-versions.json")
}

pub fn read_meta(dir: &Path) -> Option<ToolsMeta> {
    serde_json::from_slice(&std::fs::read(meta_path(dir)).ok()?).ok()
}

fn write_meta(path: &Path, m: &ToolsMeta) -> std::io::Result<()> {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(m).map_err(std::io::Error::other)?,
    )
}

/// Versions that failed their smoke test or were rolled back; never installed again.
pub fn read_bad(dir: &Path) -> Vec<String> {
    std::fs::read(bad_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn mark_bad(dir: &Path, version: &str) -> std::io::Result<()> {
    let mut v = read_bad(dir);
    if !v.iter().any(|b| b == version) {
        v.push(version.to_owned());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        bad_path(dir),
        serde_json::to_vec(&v).map_err(std::io::Error::other)?,
    )
}

/// Which executable runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chosen {
    Bundled,
    Updated { exe: PathBuf, meta: ToolsMeta },
}

/// The tools copy if its recorded version is newer than the bundled one and the file still
/// has the recorded hash; otherwise the bundled one (a later app release with a newer
/// bundled yt-dlp wins by itself).
pub fn choose(bundled_version: &str, dir: &Path) -> Chosen {
    let Some(meta) = read_meta(dir) else {
        return Chosen::Bundled;
    };
    let exe = tools_exe(dir);
    if !is_newer(&meta.version, bundled_version) {
        return Chosen::Bundled;
    }
    match sha256_file(&exe) {
        Ok(h) if h.eq_ignore_ascii_case(&meta.sha256) => Chosen::Updated { exe, meta },
        _ => Chosen::Bundled,
    }
}

/// Moves the verified, smoke-tested `staged` exe into `dir` as the active one; the active
/// one before it goes to `previous\`.
pub fn install(
    dir: &Path,
    staged: &Path,
    version: &str,
    sha256: &str,
    now: i64,
) -> std::io::Result<ToolsMeta> {
    std::fs::create_dir_all(dir)?;
    let prev = previous_dir(dir);
    let old = read_meta(dir).filter(|_| tools_exe(dir).exists());
    if let Some(old) = &old {
        let _ = std::fs::remove_dir_all(&prev);
        std::fs::create_dir_all(&prev)?;
        std::fs::rename(tools_exe(dir), tools_exe(&prev))?;
        write_meta(&meta_path(&prev), old)?;
    }
    let meta = ToolsMeta {
        version: version.to_owned(),
        sha256: sha256.to_owned(),
        installed_at: now,
        previous: old.map(|o| o.version),
    };
    // Written before the move: a crash in between leaves a meta whose hash does not match,
    // which `choose` treats as "bundled".
    write_meta(&meta_path(dir), &meta)?;
    let _ = std::fs::remove_file(tools_exe(dir));
    std::fs::rename(staged, tools_exe(dir))?;
    Ok(meta)
}

/// Drops the active updated copy and marks it bad. Puts `previous\` back as the active one
/// if there is one (returns its version), else leaves only the bundled one (`None`).
pub fn rollback(dir: &Path, bad_version: &str) -> std::io::Result<Option<String>> {
    mark_bad(dir, bad_version)?;
    let prev = previous_dir(dir);
    let restored = read_meta(&prev).filter(|_| tools_exe(&prev).exists());
    let _ = std::fs::remove_file(tools_exe(dir));
    let _ = std::fs::remove_file(meta_path(dir));
    if let Some(m) = restored {
        std::fs::rename(tools_exe(&prev), tools_exe(dir))?;
        let mut m = m;
        m.previous = None;
        write_meta(&meta_path(dir), &m)?;
        let _ = std::fs::remove_dir_all(&prev);
        return Ok(Some(m.version));
    }
    let _ = std::fs::remove_dir_all(&prev);
    Ok(None)
}

/// «Вернуть встроенную»: deletes the updated copy (and the previous one).
pub fn revert_to_bundled(dir: &Path) {
    let _ = std::fs::remove_file(tools_exe(dir));
    let _ = std::fs::remove_file(meta_path(dir));
    let _ = std::fs::remove_dir_all(previous_dir(dir));
}

/// Should this version be installed at all: newer than the one in use, parsable, not bad.
pub fn wanted(latest: &str, in_use: &str, bad: &[String]) -> bool {
    parse_version(latest).is_some() && is_newer(latest, in_use) && !bad.iter().any(|b| b == latest)
}

// ---- rollback counter ------------------------------------------------------

/// Counts consecutive batches in which the updated yt-dlp failed where the previous one
/// works.
#[derive(Debug, Default)]
pub struct RollbackCounter {
    strikes: u32,
}

impl RollbackCounter {
    /// One finished batch: `failed` ids failed in every run on the new exe; `previous_ok`:
    /// the previous exe resolved those same ids. Returns true when it is time to roll back.
    pub fn report(&mut self, failed: bool, previous_ok: bool) -> bool {
        if !failed {
            self.strikes = 0;
        } else if previous_ok {
            self.strikes += 1;
        }
        self.strikes >= ROLLBACK_STRIKES
    }

    pub fn reset(&mut self) {
        self.strikes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = include_str!("../testdata/key-a.pub.asc");
    const KEY_B: &str = include_str!("../testdata/key-b.pub.asc");
    const SUMS: &[u8] = include_bytes!("../testdata/SHA2-256SUMS");
    const SIG: &[u8] = include_bytes!("../testdata/SHA2-256SUMS.sig");

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mixtape-ytupd-{tag}-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // -- age

    #[test]
    fn age_gate() {
        let t = 1_000_000_000;
        assert!(old_enough(t, t + MIN_AGE_SECS));
        assert!(!old_enough(t, t + MIN_AGE_SECS - 1));
        assert!(!old_enough(t, t + 4 * 86_400 + 23 * 3_600));
        assert!(!old_enough(t + 10, t), "a date in the future");
        assert_eq!(install_after(t), t + MIN_AGE_SECS);
        assert!(!old_enough(i64::MAX, i64::MIN));
    }

    // -- urls and sizes

    #[test]
    fn url_allow_list() {
        assert!(allowed_url(
            "https://github.com/yt-dlp/yt-dlp/releases/download/2026.11.25/yt-dlp.exe"
        ));
        assert!(allowed_url("https://objects.githubusercontent.com/x/y"));
        assert!(allowed_url(
            "https://release-assets.githubusercontent.com/x"
        ));
        for u in [
            "http://github.com/a",
            "https://evil.com/github.com",
            "https://github.com.evil.com/a",
            "https://evilgithub.com/a",
            "https://user@github.com/a",
            "https://github.com:8443/a",
            "ftp://github.com/a",
            "file:///C:/x.exe",
            "github.com/a",
            "",
        ] {
            assert!(!allowed_url(u), "{u}");
        }
    }

    fn release(url: &str, size: u64) -> Release {
        Release {
            tag: "2026.11.25".into(),
            published_at: 0,
            assets: vec![Asset {
                name: EXE_ASSET.into(),
                url: url.into(),
                size,
            }],
        }
    }

    #[test]
    fn size_caps_and_missing_assets() {
        let ok = "https://github.com/a/yt-dlp.exe";
        assert!(pick_asset(&release(ok, MAX_EXE), EXE_ASSET, MAX_EXE).is_ok());
        assert!(pick_asset(&release(ok, MAX_EXE + 1), EXE_ASSET, MAX_EXE).is_err());
        assert!(pick_asset(&release(ok, 0), EXE_ASSET, MAX_EXE).is_err());
        assert!(pick_asset(&release("http://github.com/a", 10), EXE_ASSET, MAX_EXE).is_err());
        assert!(pick_asset(&release(ok, 10), SUMS_ASSET, MAX_SUMS).is_err());
        assert!(pick_asset(&release(ok, MAX_SUMS + 1), EXE_ASSET, MAX_SUMS).is_err());
    }

    // -- sums

    const H1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const H2: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    #[test]
    fn sums_parsing() {
        let two_files = format!("{H1}  yt-dlp\n{H2}  yt-dlp.exe\n");
        assert_eq!(
            parse_sums(&two_files, "yt-dlp.exe"),
            Some(H2.to_ascii_lowercase())
        );
        assert_eq!(
            parse_sums(&format!("{H1} *yt-dlp.exe"), "yt-dlp.exe"),
            Some(H1.into())
        );
        assert_eq!(
            parse_sums(&format!("{H1}  yt-dlp\n"), "yt-dlp.exe"),
            None,
            "missing line"
        );
        assert_eq!(
            parse_sums(
                &format!("{H1}  yt-dlp.exe\n{H2}  yt-dlp.exe\n"),
                "yt-dlp.exe"
            ),
            None,
            "two lines"
        );
        assert_eq!(
            parse_sums("zz  yt-dlp.exe", "yt-dlp.exe"),
            None,
            "short hash"
        );
        let bad = format!("{}g  yt-dlp.exe", &H1[..63]);
        assert_eq!(parse_sums(&bad, "yt-dlp.exe"), None, "bad hex");
        assert_eq!(parse_sums("", "yt-dlp.exe"), None);
        assert_eq!(
            parse_sums(&format!("{H1}  not-yt-dlp.exe"), "yt-dlp.exe"),
            None
        );
    }

    #[test]
    fn hashes() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let d = temp("hash");
        std::fs::write(d.join("f"), b"abc").unwrap();
        assert_eq!(sha256_file(&d.join("f")).unwrap(), sha256_hex(b"abc"));
        assert!(sha256_file(&d.join("nope")).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    // -- signature (fixtures made with gpg: key A signed SHA2-256SUMS)

    #[test]
    fn signature_good_tampered_wrong_key_garbage() {
        assert!(verify_signature(KEY_A, SUMS, SIG).is_ok());
        let mut tampered = SUMS.to_vec();
        tampered[0] ^= 1;
        assert!(verify_signature(KEY_A, &tampered, SIG).is_err());
        assert!(verify_signature(KEY_B, SUMS, SIG).is_err());
        assert!(verify_signature(KEY_A, SUMS, b"garbage").is_err());
        assert!(verify_signature(KEY_A, SUMS, &[]).is_err());
        assert!(verify_signature("not a key", SUMS, SIG).is_err());
    }

    #[test]
    fn verify_download_accepts_a_signed_matching_exe() {
        let sums = include_bytes!("../testdata/SHA2-256SUMS.exe-fake");
        let sig = include_bytes!("../testdata/SHA2-256SUMS.exe-fake.sig");
        let good = sha256_hex(b"fake exe");
        assert_eq!(verify_download(KEY_A, sums, sig, &good).unwrap(), good);
        assert!(verify_download(KEY_A, sums, sig, &sha256_hex(b"other exe")).is_err());
        assert!(verify_download(KEY_B, sums, sig, &good).is_err());
        // Sums signed for another file's content do not vouch for this one.
        assert!(verify_download(KEY_A, sums, SIG, &good).is_err());
    }

    #[test]
    fn verify_download_checks_signature_and_hash() {
        // The fixture sums list `abc123  yt-dlp.exe`, which is no valid SHA-256: refused
        // even though the signature is fine.
        assert!(verify_download(KEY_A, SUMS, SIG, "abc123").is_err());
        assert!(verify_download(KEY_B, SUMS, SIG, "abc123").is_err());
    }

    #[test]
    fn the_embedded_key_is_yt_dlps() {
        assert_eq!(key_fingerprint(PUBLIC_KEY).unwrap(), PUBLIC_KEY_FINGERPRINT);
        assert_ne!(key_fingerprint(KEY_A).unwrap(), PUBLIC_KEY_FINGERPRINT);
    }

    /// Manual check against a real release: `MIXTAPE_REAL_RELEASE_DIR` holds `SHA2-256SUMS`
    /// and `SHA2-256SUMS.sig` downloaded from yt-dlp's release page.
    #[test]
    #[ignore = "needs files downloaded from a real yt-dlp release"]
    fn a_real_release_verifies_with_the_embedded_key() {
        let dir = PathBuf::from(std::env::var("MIXTAPE_REAL_RELEASE_DIR").unwrap());
        let sums = std::fs::read(dir.join(SUMS_ASSET)).unwrap();
        let sig = std::fs::read(dir.join(SIG_ASSET)).unwrap();
        verify_signature(PUBLIC_KEY, &sums, &sig).unwrap();
        let mut bad = sums.clone();
        bad[0] ^= 1;
        assert!(verify_signature(PUBLIC_KEY, &bad, &sig).is_err());
        assert!(parse_sums(std::str::from_utf8(&sums).unwrap(), EXE_ASSET).is_some());
    }

    // -- which exe

    fn put(dir: &Path, bytes: &[u8], version: &str) -> ToolsMeta {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(tools_exe(dir), bytes).unwrap();
        let m = ToolsMeta {
            version: version.into(),
            sha256: sha256_hex(bytes),
            installed_at: 1,
            previous: None,
        };
        write_meta(&meta_path(dir), &m).unwrap();
        m
    }

    #[test]
    fn chooses_the_exe() {
        let d = temp("choose");
        assert_eq!(choose("2026.10.26", &d), Chosen::Bundled, "no json");
        let m = put(&d, b"new", "2026.11.25");
        assert_eq!(
            choose("2026.10.26", &d),
            Chosen::Updated {
                exe: tools_exe(&d),
                meta: m
            }
        );
        assert_eq!(choose("2026.11.25", &d), Chosen::Bundled, "equal");
        assert_eq!(
            choose("2026.12.01", &d),
            Chosen::Bundled,
            "bundled is newer"
        );
        std::fs::write(tools_exe(&d), b"tampered").unwrap();
        assert_eq!(choose("2026.10.26", &d), Chosen::Bundled, "hash mismatch");
        std::fs::remove_file(tools_exe(&d)).unwrap();
        assert_eq!(choose("2026.10.26", &d), Chosen::Bundled, "exe missing");
        let _ = std::fs::remove_dir_all(d);
    }

    // -- install and rollback

    #[test]
    fn install_then_rollback_restores_the_previous() {
        let d = temp("install");
        let stage = |bytes: &[u8]| {
            let s = staging_dir(&d);
            std::fs::create_dir_all(&s).unwrap();
            std::fs::write(tools_exe(&s), bytes).unwrap();
            tools_exe(&s)
        };
        let m1 = install(&d, &stage(b"one"), "2026.11.25", &sha256_hex(b"one"), 10).unwrap();
        assert_eq!(m1.previous, None);
        assert_eq!(std::fs::read(tools_exe(&d)).unwrap(), b"one");
        let m2 = install(&d, &stage(b"two"), "2026.12.05", &sha256_hex(b"two"), 20).unwrap();
        assert_eq!(m2.previous.as_deref(), Some("2026.11.25"));
        assert_eq!(std::fs::read(tools_exe(&d)).unwrap(), b"two");
        assert_eq!(read_meta(&d).unwrap().version, "2026.12.05");
        assert!(!tools_exe(&staging_dir(&d)).exists());

        assert_eq!(
            rollback(&d, "2026.12.05").unwrap().as_deref(),
            Some("2026.11.25")
        );
        assert_eq!(std::fs::read(tools_exe(&d)).unwrap(), b"one");
        assert_eq!(read_meta(&d).unwrap().version, "2026.11.25");
        assert_eq!(read_bad(&d), vec!["2026.12.05".to_owned()]);

        // No previous left: back to the bundled one.
        assert_eq!(rollback(&d, "2026.11.25").unwrap(), None);
        assert!(!tools_exe(&d).exists());
        assert_eq!(choose("2026.10.26", &d), Chosen::Bundled);
        assert_eq!(read_bad(&d).len(), 2);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn revert_to_bundled_deletes_the_copies() {
        let d = temp("revert");
        put(&d, b"x", "2026.11.25");
        std::fs::create_dir_all(previous_dir(&d)).unwrap();
        revert_to_bundled(&d);
        assert!(!tools_exe(&d).exists() && !meta_path(&d).exists() && !previous_dir(&d).exists());
        revert_to_bundled(&d); // nothing there: no panic
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn bad_versions() {
        let d = temp("bad");
        assert!(read_bad(&d).is_empty());
        mark_bad(&d, "2026.11.25").unwrap();
        mark_bad(&d, "2026.11.25").unwrap();
        assert_eq!(read_bad(&d), vec!["2026.11.25".to_owned()]);
        std::fs::write(bad_path(&d), b"garbage").unwrap();
        assert!(read_bad(&d).is_empty());
        let bad = vec!["2026.11.25".to_owned()];
        assert!(!wanted("2026.11.25", "2026.10.26", &bad), "bad");
        assert!(
            wanted("2026.11.26", "2026.10.26", &bad),
            "a newer one may be"
        );
        assert!(!wanted("2026.10.26", "2026.10.26", &[]), "not newer");
        assert!(!wanted("nightly", "2026.10.26", &[]), "unparsable");
        let _ = std::fs::remove_dir_all(d);
    }

    // -- rollback counter

    #[test]
    fn rollback_counter() {
        let mut c = RollbackCounter::default();
        assert!(!c.report(true, true));
        assert!(!c.report(true, true));
        assert!(c.report(true, true), "third in a row");

        let mut c = RollbackCounter::default();
        assert!(!c.report(true, true));
        assert!(!c.report(true, true));
        assert!(!c.report(false, false), "a good batch resets");
        assert!(!c.report(true, true));
        assert!(!c.report(true, true));

        let mut c = RollbackCounter::default();
        for _ in 0..10 {
            assert!(
                !c.report(true, false),
                "the old exe fails too: not the new one's fault"
            );
        }
        c.reset();
        assert!(!c.report(true, true));
    }
}
