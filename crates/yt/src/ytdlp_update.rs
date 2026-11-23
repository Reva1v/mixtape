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
use pgp::crypto::hash::HashAlgorithm;
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
    // Only SHA-2 digests: a signature over a weak digest could be forged.
    match sig.signature.hash_alg() {
        Some(HashAlgorithm::Sha256 | HashAlgorithm::Sha384 | HashAlgorithm::Sha512) => {}
        other => return Err(format!("signature: hash algorithm {other:?} not accepted")),
    }
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

/// The verified sums and their signature are kept next to every installed copy, so that
/// the copy can be checked again whenever it changes, not only when it was downloaded.
pub const SUMS_COPY: &str = "yt-dlp.SHA2-256SUMS";
pub const SIG_COPY: &str = "yt-dlp.SHA2-256SUMS.sig";
/// How many daily cycles may fail the smoke test of one version for reasons that say
/// nothing about its authenticity (network, rate limit, region) before it is left alone.
pub const MAX_SMOKE_ATTEMPTS: u32 = 3;

const RETRIES: u32 = 5;
const RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(200);

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

/// An installed copy that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub exe: PathBuf,
    pub meta: ToolsMeta,
}

/// The three downloaded files of a candidate, in a staging folder.
#[derive(Debug, Clone)]
pub struct Staged {
    pub exe: PathBuf,
    pub sums: PathBuf,
    pub sig: PathBuf,
}

impl Staged {
    /// Where [`download`]s of the release assets land in `stage`.
    pub fn in_dir(stage: &Path) -> Self {
        Self {
            exe: stage.join(EXE_ASSET),
            sums: stage.join(SUMS_ASSET),
            sig: stage.join(SIG_ASSET),
        }
    }
}

pub fn tools_exe(dir: &Path) -> PathBuf {
    dir.join(EXE_ASSET)
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("yt-dlp.json")
}

fn sums_copy(dir: &Path) -> PathBuf {
    dir.join(SUMS_COPY)
}

fn sig_copy(dir: &Path) -> PathBuf {
    dir.join(SIG_COPY)
}

fn previous_dir(dir: &Path) -> PathBuf {
    dir.join("previous")
}

pub fn staging_dir(dir: &Path) -> PathBuf {
    dir.join("staging")
}

fn bad_path(dir: &Path) -> PathBuf {
    dir.join("bad-versions.json")
}

fn reverted_path(dir: &Path) -> PathBuf {
    dir.join("reverted-versions.json")
}

fn attempts_path(dir: &Path) -> PathBuf {
    dir.join("smoke-attempts.json")
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

// -- small file helpers

/// Windows holds an exe that runs or is being scanned; try a few times.
fn retry<T>(mut f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut attempt = 1;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if attempt >= RETRIES => return Err(e),
            Err(_) => {
                attempt += 1;
                std::thread::sleep(RETRY_PAUSE);
            }
        }
    }
}

fn remove_file(p: &Path) -> std::io::Result<()> {
    retry(|| match std::fs::remove_file(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    })
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    retry(|| {
        match std::fs::remove_file(to) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        std::fs::rename(from, to)
    })
}

/// Deletes a copy. The exe goes first: if it cannot be deleted (it runs, a scanner holds it)
/// nothing else is touched, so the folder never has an exe without its description, and an
/// exe-less description is harmless.
fn remove_copy(dir: &Path) -> std::io::Result<()> {
    remove_file(&tools_exe(dir))?;
    remove_file(&sums_copy(dir))?;
    remove_file(&sig_copy(dir))?;
    remove_file(&meta_path(dir))
}

/// Moves a copy to `to`; its description is written last, so a half-moved copy is just
/// "no copy".
fn move_copy(from: &Path, to: &Path, meta: &ToolsMeta) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    move_file(&tools_exe(from), &tools_exe(to))?;
    move_file(&sums_copy(from), &sums_copy(to))?;
    move_file(&sig_copy(from), &sig_copy(to))?;
    write_meta(&meta_path(to), meta)?;
    remove_file(&meta_path(from))
}

fn read_list(p: &Path) -> Vec<String> {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn add_to_list(dir: &Path, p: &Path, version: &str) -> std::io::Result<()> {
    let mut v = read_list(p);
    if !v.iter().any(|b| b == version) {
        v.push(version.to_owned());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(p, serde_json::to_vec(&v).map_err(std::io::Error::other)?)
}

/// Versions that failed authenticity checks or were rolled back; never installed again.
pub fn read_bad(dir: &Path) -> Vec<String> {
    read_list(&bad_path(dir))
}

pub fn mark_bad(dir: &Path, version: &str) -> std::io::Result<()> {
    add_to_list(dir, &bad_path(dir), version)
}

/// Versions the user sent back with «Вернуть встроенную»; never installed again by themselves.
pub fn read_reverted(dir: &Path) -> Vec<String> {
    read_list(&reverted_path(dir))
}

pub fn mark_reverted(dir: &Path, version: &str) -> std::io::Result<()> {
    add_to_list(dir, &reverted_path(dir), version)
}

fn read_attempts(dir: &Path) -> std::collections::HashMap<String, u32> {
    std::fs::read(attempts_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Smoke-test cycles of `version` that failed for reasons other than authenticity.
pub fn attempts(dir: &Path, version: &str) -> u32 {
    read_attempts(dir).get(version).copied().unwrap_or(0)
}

/// Counts one more such cycle; returns the new count.
pub fn bump_attempts(dir: &Path, version: &str) -> u32 {
    let mut m = read_attempts(dir);
    let n = m.entry(version.to_owned()).or_insert(0);
    *n += 1;
    let n = *n;
    let _ = std::fs::create_dir_all(dir);
    if let Ok(b) = serde_json::to_vec(&m) {
        let _ = std::fs::write(attempts_path(dir), b);
    }
    n
}

/// The version has used up its smoke-test attempts: notice only from now on.
pub fn gave_up(dir: &Path, version: &str) -> bool {
    attempts(dir, version) >= MAX_SMOKE_ATTEMPTS
}

// -- verifying an installed copy

type Stamp = Option<(u64, Option<std::time::SystemTime>)>;

fn stamp(p: &Path) -> Stamp {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()))
}

fn stamps(dir: &Path) -> [Stamp; 4] {
    [
        stamp(&tools_exe(dir)),
        stamp(&meta_path(dir)),
        stamp(&sums_copy(dir)),
        stamp(&sig_copy(dir)),
    ]
}

type CacheKey = (PathBuf, String);

fn cache() -> std::sync::MutexGuard<'static, std::collections::HashMap<CacheKey, [Stamp; 4]>> {
    static C: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<CacheKey, [Stamp; 4]>>,
    > = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The full check of an installed copy: its description matches the file's SHA-256, and
/// the stored sums carry a valid signature of `armored_key` that lists that same hash.
fn verify_dir(dir: &Path, armored_key: &str) -> Result<Verified, Refusal> {
    let meta = read_meta(dir).ok_or("no description")?;
    let exe = tools_exe(dir);
    let hash = sha256_file(&exe).map_err(|e| format!("exe: {e}"))?;
    if !hash.eq_ignore_ascii_case(&meta.sha256) {
        return Err("exe differs from its description".into());
    }
    let sums = std::fs::read(sums_copy(dir)).map_err(|e| format!("sums: {e}"))?;
    let sig = std::fs::read(sig_copy(dir)).map_err(|e| format!("signature: {e}"))?;
    verify_download(armored_key, &sums, &sig, &hash)?;
    Ok(Verified { exe, meta })
}

/// The copy in `dir` if it passes every check, else `None` (and it is never to be run).
/// The result is remembered while the sizes and times of its four files stay the same, so
/// this is cheap enough to ask before every use; any change checks it all again.
pub fn verified(dir: &Path, armored_key: &str) -> Option<Verified> {
    let key = (dir.to_path_buf(), sha256_hex(armored_key.as_bytes()));
    let now = stamps(dir);
    if now.iter().any(Option::is_none) {
        cache().remove(&key);
        return None;
    }
    if cache().get(&key) == Some(&now) {
        return Some(Verified {
            exe: tools_exe(dir),
            meta: read_meta(dir)?,
        });
    }
    match verify_dir(dir, armored_key) {
        Ok(v) => {
            cache().insert(key, now);
            Some(v)
        }
        Err(why) => {
            cache().remove(&key);
            tracing::info!(dir = %dir.display(), "yt-dlp copy refused: {why}");
            None
        }
    }
}

/// Which executable runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chosen {
    Bundled,
    Updated { exe: PathBuf, meta: ToolsMeta },
}

/// The tools copy if it passes every check, its version is newer than the bundled one and
/// was not refused or sent back; otherwise the bundled one (a later app release with a
/// newer bundled yt-dlp wins by itself).
pub fn choose(bundled_version: &str, dir: &Path, armored_key: &str) -> Chosen {
    let Some(v) = verified(dir, armored_key) else {
        return Chosen::Bundled;
    };
    let v_str = v.meta.version.as_str();
    if !is_newer(v_str, bundled_version)
        || read_bad(dir).iter().any(|b| b == v_str)
        || read_reverted(dir).iter().any(|b| b == v_str)
    {
        return Chosen::Bundled;
    }
    Chosen::Updated {
        exe: v.exe,
        meta: v.meta,
    }
}

/// **Entry point for other programs** (Droplet Island): the yt-dlp executable to run, given
/// Mixtape's tools folder (`%LOCALAPPDATA%\Mixtape\tools`), the bundled exe and its version.
///
/// The updated copy in `tools_dir` is returned only if it passes every check: its stored
/// SHA2-256SUMS carries a valid signature of the key embedded in this crate (SHA-2 digest
/// only), the sums line equals the file's SHA-256 and the description's, its version is newer
/// than `bundled_version`, and the version was neither refused nor sent back by the user.
/// Anything off (no copy, a changed file, a missing companion, a bad version) gives
/// `bundled_exe`. It only reads: it never downloads, writes or deletes anything, and a
/// verified copy is remembered while its files keep their sizes and times, so asking before
/// every spawn is cheap.
pub fn exe_to_run(tools_dir: &Path, bundled_exe: &Path, bundled_version: &str) -> PathBuf {
    exe_to_run_with_key(tools_dir, bundled_exe, bundled_version, PUBLIC_KEY)
}

/// [`exe_to_run`] with the signing key given (tests use their own).
pub fn exe_to_run_with_key(
    tools_dir: &Path,
    bundled_exe: &Path,
    bundled_version: &str,
    armored_key: &str,
) -> PathBuf {
    match choose(bundled_version, tools_dir, armored_key) {
        Chosen::Updated { exe, .. } => exe,
        Chosen::Bundled => bundled_exe.to_path_buf(),
    }
}

/// The previous copy, if it passes every check and is newer than the bundled yt-dlp (an
/// older one is never worth going back to).
pub fn previous_verified(dir: &Path, bundled_version: &str, armored_key: &str) -> Option<Verified> {
    verified(&previous_dir(dir), armored_key).filter(|v| is_newer(&v.meta.version, bundled_version))
}

/// Moves the verified, smoke-tested `staged` files into `dir` as the active copy. The copy
/// that ran until now goes to `previous\` only if `in_use_updated` (it really was the one in
/// use, not a stale file) and it still passes every check; if the bundled yt-dlp was in use
/// there is no previous copy, and a rollback means the bundled one. The description is
/// written last: any failure in between leaves "no copy".
pub fn install(
    dir: &Path,
    staged: &Staged,
    version: &str,
    sha256: &str,
    now: i64,
    in_use_updated: bool,
    armored_key: &str,
) -> std::io::Result<ToolsMeta> {
    std::fs::create_dir_all(dir)?;
    let prev = previous_dir(dir);
    let old = if in_use_updated {
        verified(dir, armored_key)
    } else {
        None
    };
    let _ = std::fs::remove_dir_all(&prev);
    match &old {
        Some(o) => move_copy(dir, &prev, &o.meta)?,
        None => remove_copy(dir)?,
    }
    let meta = ToolsMeta {
        version: version.to_owned(),
        sha256: sha256.to_owned(),
        installed_at: now,
        previous: old.map(|o| o.meta.version),
    };
    move_file(&staged.exe, &tools_exe(dir))?;
    move_file(&staged.sums, &sums_copy(dir))?;
    move_file(&staged.sig, &sig_copy(dir))?;
    write_meta(&meta_path(dir), &meta)?;
    Ok(meta)
}

/// Drops the active updated copy, marks it bad, and puts `previous\` back as the active one
/// if that passes every check and is newer than the bundled yt-dlp (returns its version);
/// otherwise only the bundled one is left (`None`). The caller has already pointed the
/// resolver at a safe exe; if a file cannot be removed this is an `Err`, the version stays
/// marked bad (so it is not chosen at the next start) and the bundled one keeps running.
pub fn rollback(
    dir: &Path,
    bad_version: &str,
    bundled_version: &str,
    armored_key: &str,
) -> std::io::Result<Option<String>> {
    mark_bad(dir, bad_version)?;
    let prev = previous_dir(dir);
    let target = previous_verified(dir, bundled_version, armored_key);
    remove_copy(dir)?;
    let Some(t) = target else {
        let _ = std::fs::remove_dir_all(&prev);
        return Ok(None);
    };
    let mut meta = t.meta;
    meta.previous = None;
    move_copy(&prev, dir, &meta)?;
    let _ = std::fs::remove_dir_all(&prev);
    Ok(Some(meta.version))
}

/// «Вернуть встроенную»: deletes the updated copy and the previous one, and remembers the
/// version the user sent back so it is not installed again (a newer one may be). The caller
/// has already pointed the resolver at the bundled exe.
pub fn revert_to_bundled(dir: &Path, version_in_use: Option<&str>) -> std::io::Result<()> {
    if let Some(v) = version_in_use {
        mark_reverted(dir, v)?;
    }
    remove_copy(dir)?;
    let _ = std::fs::remove_dir_all(previous_dir(dir));
    Ok(())
}

/// Should this version be installed at all: parsable, newer than the one in use, not
/// refused and not sent back by the user.
pub fn wanted(latest: &str, in_use: &str, bad: &[String], reverted: &[String]) -> bool {
    parse_version(latest).is_some()
        && is_newer(latest, in_use)
        && !bad.iter().any(|b| b == latest)
        && !reverted.iter().any(|b| b == latest)
}

/// The newest wanted release that is old enough (the latest one may still be too young
/// while the one before it is fine).
pub fn pick_candidate<'a>(
    releases: &'a [Release],
    in_use: &str,
    bad: &[String],
    reverted: &[String],
    now: i64,
) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| wanted(&r.tag, in_use, bad, reverted) && old_enough(r.published_at, now))
        .max_by_key(|r| parse_version(&r.tag))
}

/// Why a candidate did not become the active yt-dlp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// It is not what it claims to be (signature, hash, version string): never again.
    Permanent(Refusal),
    /// Nothing wrong with the file as far as we can tell (network, rate limit, region,
    /// timeout): try again next cycle, a few times.
    Transient(Refusal),
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

    // -- hash algorithm of the signature

    #[test]
    fn only_sha2_signatures_are_accepted() {
        // The same sums signed by the same key with SHA-512 (the default fixture), SHA-1 and
        // SHA-224.
        assert!(verify_signature(KEY_A, SUMS, SIG).is_ok());
        let sha1 = include_bytes!("../testdata/SHA2-256SUMS.sha1.sig");
        let sha224 = include_bytes!("../testdata/SHA2-256SUMS.sha224.sig");
        let e1 = verify_signature(KEY_A, SUMS, sha1).unwrap_err();
        assert!(e1.contains("hash algorithm"), "{e1}");
        let e2 = verify_signature(KEY_A, SUMS, sha224).unwrap_err();
        assert!(e2.contains("hash algorithm"), "{e2}");
    }

    // -- signed fixtures: key A signed the sums of these three "exes"

    struct Set {
        exe: &'static [u8],
        sums: &'static [u8],
        sig: &'static [u8],
    }

    const ONE: Set = Set {
        exe: b"fake exe",
        sums: include_bytes!("../testdata/SHA2-256SUMS.exe-fake"),
        sig: include_bytes!("../testdata/SHA2-256SUMS.exe-fake.sig"),
    };
    const TWO: Set = Set {
        exe: b"exe two",
        sums: include_bytes!("../testdata/SHA2-256SUMS.exe-two"),
        sig: include_bytes!("../testdata/SHA2-256SUMS.exe-two.sig"),
    };

    /// A genuine installed copy of `set` as `version`.
    fn put(dir: &Path, set: &Set, version: &str) -> ToolsMeta {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(tools_exe(dir), set.exe).unwrap();
        std::fs::write(sums_copy(dir), set.sums).unwrap();
        std::fs::write(sig_copy(dir), set.sig).unwrap();
        let m = ToolsMeta {
            version: version.into(),
            sha256: sha256_hex(set.exe),
            installed_at: 1,
            previous: None,
        };
        write_meta(&meta_path(dir), &m).unwrap();
        m
    }

    fn stage(d: &Path, set: &Set) -> Staged {
        let s = Staged::in_dir(&staging_dir(d));
        std::fs::create_dir_all(staging_dir(d)).unwrap();
        std::fs::write(&s.exe, set.exe).unwrap();
        std::fs::write(&s.sums, set.sums).unwrap();
        std::fs::write(&s.sig, set.sig).unwrap();
        s
    }

    fn do_install(d: &Path, set: &Set, v: &str, in_use_updated: bool) -> ToolsMeta {
        install(
            d,
            &stage(d, set),
            v,
            &sha256_hex(set.exe),
            10,
            in_use_updated,
            KEY_A,
        )
        .unwrap()
    }

    // -- verifying an installed copy

    #[test]
    fn a_genuine_copy_verifies() {
        let d = temp("verify");
        let m = put(&d, &ONE, "2026.11.25");
        let v = verified(&d, KEY_A).unwrap();
        assert_eq!(v.meta, m);
        assert_eq!(v.exe, tools_exe(&d));
        assert!(verified(&d, KEY_B).is_none(), "another key");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_copy_that_is_not_exactly_as_signed_never_verifies() {
        // exe replaced, json and hash consistent with each other but not signed
        let d = temp("forged");
        put(&d, &ONE, "2026.11.25");
        assert!(verified(&d, KEY_A).is_some());
        std::fs::write(tools_exe(&d), b"evil exe").unwrap();
        let mut m = read_meta(&d).unwrap();
        m.sha256 = sha256_hex(b"evil exe");
        write_meta(&meta_path(&d), &m).unwrap();
        assert!(verified(&d, KEY_A).is_none(), "forged json + exe pair");

        // sums edited to list the forged hash: the signature no longer matches
        let forged = format!("{}  yt-dlp.exe\n", sha256_hex(b"evil exe"));
        std::fs::write(sums_copy(&d), forged).unwrap();
        assert!(verified(&d, KEY_A).is_none(), "forged sums");

        // json hash edited only
        let d2 = temp("forged2");
        put(&d2, &ONE, "2026.11.25");
        let mut m = read_meta(&d2).unwrap();
        m.sha256 = sha256_hex(b"other");
        write_meta(&meta_path(&d2), &m).unwrap();
        assert!(verified(&d2, KEY_A).is_none(), "json hash differs from the file");

        // exe swapped for another signed exe: its own sums are not in the folder
        let d3 = temp("swap");
        put(&d3, &ONE, "2026.11.25");
        std::fs::write(tools_exe(&d3), TWO.exe).unwrap();
        assert!(verified(&d3, KEY_A).is_none(), "another genuine exe under this json");

        // missing companions
        let d4 = temp("missing");
        put(&d4, &ONE, "2026.11.25");
        std::fs::remove_file(sig_copy(&d4)).unwrap();
        assert!(verified(&d4, KEY_A).is_none(), "no signature copy");
        std::fs::remove_file(sums_copy(&d4)).unwrap();
        assert!(verified(&d4, KEY_A).is_none(), "no sums copy");
        for x in [d, d2, d3, d4] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    #[test]
    fn a_change_of_size_or_time_is_checked_again() {
        let d = temp("cache");
        put(&d, &ONE, "2026.11.25");
        assert!(verified(&d, KEY_A).is_some());
        assert!(verified(&d, KEY_A).is_some(), "remembered");
        // same length, other content
        std::fs::write(tools_exe(&d), b"fake exf").unwrap();
        assert!(verified(&d, KEY_A).is_none(), "same size, new time");
        // back to the genuine bytes: checked again and fine
        std::fs::write(tools_exe(&d), ONE.exe).unwrap();
        assert!(verified(&d, KEY_A).is_some());
        // only the time moves
        let f = std::fs::OpenOptions::new().write(true).open(tools_exe(&d)).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
            .unwrap();
        drop(f);
        assert!(verified(&d, KEY_A).is_some(), "re-checked and still genuine");
        // now corrupt while keeping size, then fix the time to look "new" again
        std::fs::write(tools_exe(&d), b"fake exf").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(tools_exe(&d)).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(120))
            .unwrap();
        drop(f);
        assert!(verified(&d, KEY_A).is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    // -- which exe

    #[test]
    fn chooses_the_exe() {
        let d = temp("choose");
        assert_eq!(choose("2026.10.26", &d, KEY_A), Chosen::Bundled, "no copy");
        let m = put(&d, &ONE, "2026.11.25");
        assert_eq!(
            choose("2026.10.26", &d, KEY_A),
            Chosen::Updated {
                exe: tools_exe(&d),
                meta: m
            }
        );
        assert_eq!(choose("2026.11.25", &d, KEY_A), Chosen::Bundled, "equal");
        assert_eq!(
            choose("2026.12.01", &d, KEY_A),
            Chosen::Bundled,
            "bundled is newer"
        );
        assert_eq!(choose("2026.10.26", &d, KEY_B), Chosen::Bundled, "wrong key");
        mark_bad(&d, "2026.11.25").unwrap();
        assert_eq!(choose("2026.10.26", &d, KEY_A), Chosen::Bundled, "bad version");
        let d2 = temp("choose2");
        put(&d2, &ONE, "2026.11.25");
        mark_reverted(&d2, "2026.11.25").unwrap();
        assert_eq!(choose("2026.10.26", &d2, KEY_A), Chosen::Bundled, "sent back");
        let d3 = temp("choose3");
        put(&d3, &ONE, "2026.11.25");
        std::fs::write(tools_exe(&d3), b"tampered").unwrap();
        assert_eq!(choose("2026.10.26", &d3, KEY_A), Chosen::Bundled, "tampered");
        std::fs::remove_file(tools_exe(&d3)).unwrap();
        assert_eq!(choose("2026.10.26", &d3, KEY_A), Chosen::Bundled, "exe missing");
        for x in [d, d2, d3] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    // -- the entry point for other programs

    #[test]
    fn exe_to_run_gives_the_updated_exe_only_when_everything_checks_out() {
        let bundled = PathBuf::from("C:/app/yt-dlp.exe");
        let run = |d: &Path, bv: &str, key: &str| exe_to_run_with_key(d, &bundled, bv, key);

        let d = temp("entry");
        assert_eq!(run(&d, "2026.10.26", KEY_A), bundled, "no copy");
        put(&d, &ONE, "2026.11.25");
        assert_eq!(run(&d, "2026.10.26", KEY_A), tools_exe(&d), "genuine and newer");
        assert_eq!(run(&d, "2026.11.25", KEY_A), bundled, "not newer than bundled");
        assert_eq!(run(&d, "2026.12.01", KEY_A), bundled, "bundled is newer");
        assert_eq!(run(&d, "2026.10.26", KEY_B), bundled, "not signed by this key");

        // the public entry point uses the embedded yt-dlp key: the test key is not it
        assert_eq!(exe_to_run(&d, &bundled, "2026.10.26"), bundled);

        // read-only: nothing appeared or changed while asking
        let before: Vec<_> = {
            let mut v: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
            v.sort();
            v
        };
        let _ = run(&d, "2026.10.26", KEY_A);
        let after: Vec<_> = {
            let mut v: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
            v.sort();
            v
        };
        assert_eq!(before, after);

        mark_bad(&d, "2026.11.25").unwrap();
        assert_eq!(run(&d, "2026.10.26", KEY_A), bundled, "refused version");
        let _ = std::fs::remove_dir_all(&d);

        let d2 = temp("entry2");
        put(&d2, &ONE, "2026.11.25");
        mark_reverted(&d2, "2026.11.25").unwrap();
        assert_eq!(run(&d2, "2026.10.26", KEY_A), bundled, "sent back by the user");

        let d3 = temp("entry3");
        put(&d3, &ONE, "2026.11.25");
        std::fs::write(tools_exe(&d3), b"evil exe").unwrap();
        assert_eq!(run(&d3, "2026.10.26", KEY_A), bundled, "exe replaced");
        let mut m = read_meta(&d3).unwrap();
        m.sha256 = sha256_hex(b"evil exe");
        write_meta(&meta_path(&d3), &m).unwrap();
        assert_eq!(run(&d3, "2026.10.26", KEY_A), bundled, "forged json + exe");

        let d4 = temp("entry4");
        put(&d4, &ONE, "2026.11.25");
        std::fs::remove_file(sig_copy(&d4)).unwrap();
        assert_eq!(run(&d4, "2026.10.26", KEY_A), bundled, "signature copy missing");
        assert_eq!(run(&temp("entry5"), "2026.10.26", KEY_A), bundled, "empty folder");
        assert_eq!(
            run(Path::new("Z:/no/such/dir"), "2026.10.26", KEY_A),
            bundled,
            "no folder"
        );
        for x in [d2, d3, d4] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    // -- install and rollback

    #[test]
    fn install_keeps_the_exe_that_ran_as_previous() {
        let d = temp("install");
        let m1 = do_install(&d, &ONE, "2026.11.25", false);
        assert_eq!(m1.previous, None);
        assert!(!previous_dir(&d).exists(), "bundled was in use: nothing to keep");
        assert_eq!(std::fs::read(tools_exe(&d)).unwrap(), ONE.exe);

        let m2 = do_install(&d, &TWO, "2026.12.05", true);
        assert_eq!(m2.previous.as_deref(), Some("2026.11.25"));
        assert_eq!(std::fs::read(tools_exe(&d)).unwrap(), TWO.exe);
        assert_eq!(read_meta(&d).unwrap().version, "2026.12.05");
        assert!(!tools_exe(&staging_dir(&d)).exists());
        assert!(verified(&d, KEY_A).is_some());
        let p = previous_verified(&d, "2026.10.26", KEY_A).unwrap();
        assert_eq!(p.meta.version, "2026.11.25");
        assert_eq!(std::fs::read(&p.exe).unwrap(), ONE.exe);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_stale_or_tampered_copy_does_not_become_previous() {
        // A copy lying in tools\ that was not in use (the bundled one was) is not kept.
        let d = temp("stale");
        put(&d, &ONE, "2026.11.25");
        do_install(&d, &TWO, "2026.12.05", false);
        assert!(previous_verified(&d, "2026.10.26", KEY_A).is_none());
        assert!(!previous_dir(&d).exists());

        // The copy in use was modified since: it never goes to previous\.
        let d2 = temp("tampered");
        put(&d2, &ONE, "2026.11.25");
        std::fs::write(tools_exe(&d2), b"evil").unwrap();
        do_install(&d2, &TWO, "2026.12.05", true);
        assert!(previous_verified(&d2, "2026.10.26", KEY_A).is_none());
        assert_eq!(read_meta(&d2).unwrap().previous, None);
        assert_eq!(std::fs::read(tools_exe(&d2)).unwrap(), TWO.exe);
        for x in [d, d2] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    #[test]
    fn rollback_restores_a_verified_newer_previous() {
        let d = temp("rb");
        do_install(&d, &ONE, "2026.11.25", false);
        do_install(&d, &TWO, "2026.12.05", true);
        let r = rollback(&d, "2026.12.05", "2026.10.26", KEY_A).unwrap();
        assert_eq!(r.as_deref(), Some("2026.11.25"));
        assert_eq!(std::fs::read(tools_exe(&d)).unwrap(), ONE.exe);
        assert_eq!(read_meta(&d).unwrap().version, "2026.11.25");
        assert_eq!(read_meta(&d).unwrap().previous, None);
        assert_eq!(read_bad(&d), vec!["2026.12.05".to_owned()]);
        assert!(verified(&d, KEY_A).is_some(), "the restored copy is verifiable");
        assert!(!previous_dir(&d).exists());

        // nothing left to go back to: only the bundled one remains
        assert_eq!(rollback(&d, "2026.11.25", "2026.10.26", KEY_A).unwrap(), None);
        assert!(!tools_exe(&d).exists() && !meta_path(&d).exists());
        assert_eq!(choose("2026.10.26", &d, KEY_A), Chosen::Bundled);
        assert_eq!(read_bad(&d).len(), 2);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rollback_never_runs_an_unverified_or_older_previous() {
        // previous tampered
        let d = temp("rb-tamper");
        do_install(&d, &ONE, "2026.11.25", false);
        do_install(&d, &TWO, "2026.12.05", true);
        std::fs::write(tools_exe(&previous_dir(&d)), b"evil").unwrap();
        assert_eq!(rollback(&d, "2026.12.05", "2026.10.26", KEY_A).unwrap(), None);
        assert!(!tools_exe(&d).exists(), "nothing unverified is put in place");
        assert!(!previous_dir(&d).exists());

        // previous older than the bundled exe
        let d2 = temp("rb-old");
        do_install(&d2, &ONE, "2026.11.25", false);
        do_install(&d2, &TWO, "2026.12.05", true);
        assert_eq!(
            rollback(&d2, "2026.12.05", "2026.12.01", KEY_A).unwrap(),
            None,
            "bundled 2026.12.01 is newer than previous 2026.11.25"
        );
        assert!(!tools_exe(&d2).exists());
        for x in [d, d2] {
            let _ = std::fs::remove_dir_all(x);
        }
    }

    #[cfg(windows)]
    #[test]
    fn rollback_with_a_locked_exe_fails_without_splitting_the_copy() {
        use std::os::windows::fs::OpenOptionsExt as _;
        let d = temp("rb-lock");
        do_install(&d, &ONE, "2026.11.25", false);
        do_install(&d, &TWO, "2026.12.05", true);
        // an open handle that shares nothing, like a running exe
        let _held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(tools_exe(&d))
            .unwrap();
        assert!(rollback(&d, "2026.12.05", "2026.10.26", KEY_A).is_err());
        assert!(tools_exe(&d).exists(), "exe still there");
        assert!(meta_path(&d).exists(), "and its description with it");
        assert_eq!(read_bad(&d), vec!["2026.12.05".to_owned()], "never chosen again");
        assert_eq!(choose("2026.10.26", &d, KEY_A), Chosen::Bundled);
        drop(_held);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn revert_to_bundled_sticks_to_that_version() {
        let d = temp("revert");
        do_install(&d, &ONE, "2026.11.25", false);
        do_install(&d, &TWO, "2026.12.05", true);
        revert_to_bundled(&d, Some("2026.12.05")).unwrap();
        assert!(!tools_exe(&d).exists() && !meta_path(&d).exists() && !previous_dir(&d).exists());
        assert_eq!(read_reverted(&d), vec!["2026.12.05".to_owned()]);
        let rev = read_reverted(&d);
        assert!(!wanted("2026.12.05", "2026.10.26", &[], &rev), "never again by itself");
        assert!(wanted("2026.12.06", "2026.10.26", &[], &rev), "a newer one may install");
        revert_to_bundled(&d, None).unwrap(); // nothing there: fine
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
        assert!(!wanted("2026.11.25", "2026.10.26", &bad, &[]), "bad");
        assert!(wanted("2026.11.26", "2026.10.26", &bad, &[]), "a newer one may be");
        assert!(!wanted("2026.10.26", "2026.10.26", &[], &[]), "not newer");
        assert!(!wanted("nightly", "2026.10.26", &[], &[]), "unparsable");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn smoke_attempts_are_capped_per_version() {
        let d = temp("attempts");
        assert_eq!(attempts(&d, "2026.11.25"), 0);
        assert!(!gave_up(&d, "2026.11.25"));
        assert_eq!(bump_attempts(&d, "2026.11.25"), 1);
        assert_eq!(bump_attempts(&d, "2026.11.25"), 2);
        assert!(!gave_up(&d, "2026.11.25"));
        assert_eq!(bump_attempts(&d, "2026.11.25"), MAX_SMOKE_ATTEMPTS);
        assert!(gave_up(&d, "2026.11.25"));
        assert!(!gave_up(&d, "2026.12.01"), "another version starts fresh");
        std::fs::write(attempts_path(&d), b"garbage").unwrap();
        assert_eq!(attempts(&d, "2026.11.25"), 0);
        let _ = std::fs::remove_dir_all(d);
    }

    // -- candidate choice

    fn rel(tag: &str, published_at: i64) -> Release {
        Release {
            tag: tag.into(),
            published_at,
            assets: vec![],
        }
    }

    #[test]
    fn candidate_is_the_newest_old_enough_release() {
        let now = 100 * 86_400;
        let rels = vec![
            rel("2026.11.25", now - 86_400),      // newest, one day old
            rel("2026.11.20", now - 6 * 86_400),  // eligible
            rel("2026.11.10", now - 15 * 86_400), // eligible, older
            rel("2026.10.26", now - 30 * 86_400), // in use
        ];
        let pick = |in_use: &str, bad: &[String], rev: &[String]| {
            pick_candidate(&rels, in_use, bad, rev, now).map(|r| r.tag.clone())
        };
        assert_eq!(pick("2026.10.26", &[], &[]).as_deref(), Some("2026.11.20"));
        assert_eq!(
            pick("2026.10.26", &["2026.11.20".into()], &[]).as_deref(),
            Some("2026.11.10")
        );
        assert_eq!(
            pick("2026.10.26", &[], &["2026.11.20".into()]).as_deref(),
            Some("2026.11.10")
        );
        assert_eq!(pick("2026.11.20", &[], &[]), None, "nothing newer and old enough");
        assert_eq!(pick("2026.11.25", &[], &[]), None);
        // exactly 5 days counts
        let r5 = [rel("2026.11.25", now - MIN_AGE_SECS)];
        assert!(pick_candidate(&r5, "2026.10.26", &[], &[], now).is_some());
        let r4 = [rel("2026.11.25", now - MIN_AGE_SECS + 1)];
        assert!(pick_candidate(&r4, "2026.10.26", &[], &[], now).is_none());
        assert!(pick_candidate(&[], "2026.10.26", &[], &[], now).is_none());
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
