//! Updates from GitHub Releases (`latest.json` next to the signed installer). A check runs
//! a minute after start and then once a day while the setting is on; a found version is
//! announced to the UI, which offers to install it. Installing downloads the installer,
//! runs it in passive mode and closes the app.

use mixtape_session::commands::CmdResult;
use mixtape_session::commands::settings::Prefs;
use mixtape_session::Session;
use mixtape_core::{AppError, ErrorKind, TrackId};
use mixtape_yt::ytdlp_release as release;
use mixtape_yt::ytdlp_update as tools;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;
use ts_rs::TS;

const FIRST_CHECK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateInfo {
    pub version: String,
    /// Release notes, if the release has any.
    #[ts(optional)]
    pub notes: Option<String>,
}

fn failed(e: impl std::fmt::Display) -> AppError {
    AppError::new(
        ErrorKind::Network,
        mixtape_core::trf!("Couldn't check for updates: {e}", "Не вдалося перевірити оновлення: {e}", "Не удалось проверить обновления: {e}"),
    )
}

async fn find(app: &AppHandle) -> CmdResult<Option<tauri_plugin_updater::Update>> {
    app.updater().map_err(failed)?.check().await.map_err(failed)
}

pub fn spawn(st: Arc<Session>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if Prefs::load(&st).auto_update {
                match find(&st.app).await {
                    Ok(Some(u)) => {
                        let _ = st.app.emit("update:available", UpdateInfo { version: u.version, notes: u.body });
                    }
                    Ok(None) => {}
                    // Offline, or the release feed isn't reachable: try again tomorrow.
                    Err(e) => tracing::info!("update check: {}", e.message),
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> CmdResult<Option<UpdateInfo>> {
    Ok(find(&app).await?.map(|u| UpdateInfo { version: u.version, notes: u.body }))
}

/// Downloads and starts the installer; reports `update:progress` {done, total}. The
/// installer closes the app, so on success this does not return to the UI.
#[tauri::command]
pub async fn update_install(app: AppHandle, state: State<'_, Arc<Session>>) -> CmdResult<()> {
    let update = find(&app).await?.ok_or_else(|| {
        AppError::new(ErrorKind::NotFound, mixtape_core::tr("No update available", "Оновлень немає", "Обновлений нет"))
    })?;
    // The playback position is saved with the queue, so the new version picks up there.
    if let Some(p) = state.running_player() {
        p.pause();
    }
    let handle = app.clone();
    let mut done = 0u64;
    update
        .download_and_install(
            move |chunk, total| {
                done += chunk as u64;
                let _ = handle.emit("update:progress", serde_json::json!({ "done": done, "total": total }));
            },
            || {},
        )
        .await
        .map_err(failed)?;
    app.restart();
}

// ---- yt-dlp: notice, delayed verified update, rollback ----------------------------------

/// A public video for the smoke test of a candidate yt-dlp.
const SMOKE_ID: &str = "dQw4w9WgXcQ";

/// A new stable yt-dlp exists (`ytdlp:available`).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct YtdlpInfo {
    /// The version in use.
    pub installed: String,
    pub latest: String,
    /// When it installs itself (Unix seconds); absent when the update setting is off.
    #[ts(optional, type = "number")]
    pub install_after: Option<i64>,
}

/// yt-dlp was installed (`ytdlp:installed`) or rolled back (`ytdlp:rolledback`).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct YtdlpChange {
    /// The version now in use, or the one that was installed.
    pub version: String,
    /// On a rollback: the version that did not work.
    #[ts(optional)]
    pub bad: Option<String>,
}

/// What Settings shows.
#[derive(Debug, Clone, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct YtdlpStatus {
    /// The version in use; absent when it could not be read.
    #[ts(optional)]
    pub installed: Option<String>,
    /// An updated copy runs (otherwise the bundled one).
    pub updated: bool,
    /// When the updated copy was installed (Unix seconds).
    #[ts(optional, type = "number")]
    pub updated_at: Option<i64>,
    /// The latest stable release, when a check found it.
    #[ts(optional)]
    pub latest: Option<String>,
    pub newer: bool,
    /// When the newer one installs itself (Unix seconds), if it will.
    #[ts(optional, type = "number")]
    pub install_after: Option<i64>,
    /// A version the user sent back with «Вернуть встроенную»; it is not installed again.
    #[ts(optional)]
    pub reverted: Option<String>,
}

#[derive(Default)]
struct YtdlpRt {
    /// Version of the bundled yt-dlp, read once.
    bundled_version: Option<String>,
    in_use: Option<String>,
    updated_at: Option<i64>,
    latest: Option<String>,
    install_after: Option<i64>,
    /// Versions the user sent back (a copy of the list on disk).
    reverted: Vec<String>,
    counter: tools::RollbackCounter,
}

fn rt() -> std::sync::MutexGuard<'static, YtdlpRt> {
    static RT: OnceLock<Mutex<YtdlpRt>> = OnceLock::new();
    RT.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// One check, install or rollback at a time.
fn busy() -> &'static tokio::sync::Mutex<()> {
    static BUSY: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    BUSY.get_or_init(Default::default)
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn tools_dir(st: &Session) -> PathBuf {
    st.paths.root.join("tools")
}

fn status() -> YtdlpStatus {
    let r = rt();
    let newer = match (&r.latest, &r.in_use) {
        (Some(l), Some(i)) => release::is_newer(l, i),
        _ => false,
    };
    let held_back = r.latest.as_ref().is_some_and(|l| r.reverted.contains(l));
    // The newest version the user sent back, while it is still newer than what runs.
    let reverted = r
        .reverted
        .iter()
        .filter(|v| r.in_use.as_ref().is_some_and(|i| release::is_newer(v, i)))
        .max_by_key(|v| release::parse_version(v))
        .cloned();
    YtdlpStatus {
        installed: r.in_use.clone(),
        updated: r.updated_at.is_some(),
        updated_at: r.updated_at,
        latest: r.latest.clone(),
        newer,
        install_after: if newer && !held_back { r.install_after } else { None },
        reverted,
    }
}

/// Decides which exe runs by the full check of the tools copy (signature, hash, version,
/// not refused, not sent back) and points the resolver at it, or at the bundled one. Also
/// called before every cycle, which re-verifies a copy whose files changed.
fn apply_choice(st: &Session) {
    let Some(bundled_version) = rt().bundled_version.clone() else { return };
    let dir = tools_dir(st);
    let chosen = tools::choose(&bundled_version, &dir, tools::PUBLIC_KEY);
    let reverted = tools::read_reverted(&dir);
    let mut r = rt();
    r.reverted = reverted;
    match chosen {
        tools::Chosen::Updated { exe, meta } => {
            if r.in_use.as_deref() != Some(meta.version.as_str()) || st.resolver.override_generation().is_none() {
                tracing::info!(version = %meta.version, bundled = %bundled_version, "using the updated yt-dlp");
                st.resolver.set_ytdlp_override(Some(exe));
                r.counter.reset();
            }
            r.in_use = Some(meta.version);
            r.updated_at = Some(meta.installed_at);
        }
        tools::Chosen::Bundled => {
            if r.updated_at.is_some() || st.resolver.override_generation().is_some() {
                tracing::info!("the updated yt-dlp is not usable, using the bundled one");
                st.resolver.set_ytdlp_override(None);
            }
            r.in_use = Some(bundled_version);
            r.updated_at = None;
        }
    }
}

/// Reads the bundled yt-dlp's version once, then applies the choice.
async fn ensure_in_use(st: &Session) {
    if rt().bundled_version.is_none() {
        let bundled = st.resolver.bundled_ytdlp().to_path_buf();
        match release::installed_version(&bundled).await {
            Ok(v) => rt().bundled_version = Some(v),
            Err(e) => {
                tracing::info!("yt-dlp version: {e}");
                return;
            }
        }
    }
    apply_choice(st);
}

/// Downloads, verifies, smoke-tests and installs `rel`; `Err` is the reason it did not become
/// the active yt-dlp. Authenticity failures (signature, hash, version string) mark the
/// version bad for good; a smoke test that fails for any other reason is retried in the
/// next cycles, a few times.
async fn install_release(st: &Session, rel: &release::Release) -> Result<(), String> {
    let dir = tools_dir(st);
    let stage = tools::staging_dir(&dir);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| format!("staging folder: {e}"))?;
    let result = verify_and_install(st, rel, &dir, &stage).await;
    let _ = std::fs::remove_dir_all(&stage);
    result
}

async fn verify_and_install(st: &Session, rel: &release::Release, dir: &std::path::Path, stage: &std::path::Path) -> Result<(), String> {
    let exe = tools::pick_asset(rel, tools::EXE_ASSET, tools::MAX_EXE)?;
    let sums = tools::pick_asset(rel, tools::SUMS_ASSET, tools::MAX_SUMS)?;
    let sig = tools::pick_asset(rel, tools::SIG_ASSET, tools::MAX_SIG)?;
    let http = tools::download_client()?;
    let staged = tools::Staged::in_dir(stage);
    tools::download(&http, sums, tools::MAX_SUMS, &staged.sums).await?;
    tools::download(&http, sig, tools::MAX_SIG, &staged.sig).await?;
    tools::download(&http, exe, tools::MAX_EXE, &staged.exe).await?;
    tracing::info!(version = %rel.tag, "yt-dlp downloaded, verifying");
    let read = |p: &std::path::Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let hash = tools::sha256_file(&staged.exe).map_err(|e| format!("hash: {e}"))?;
    let hash = match tools::verify_download(tools::PUBLIC_KEY, &read(&staged.sums)?, &read(&staged.sig)?, &hash) {
        Ok(h) => h,
        Err(e) => {
            let _ = tools::mark_bad(dir, &rel.tag);
            return Err(e);
        }
    };
    if let Err(f) = smoke_test(st, &staged.exe, &rel.tag).await {
        return Err(match f {
            tools::Failure::Permanent(e) => {
                let _ = tools::mark_bad(dir, &rel.tag);
                e
            }
            tools::Failure::Transient(e) => {
                let n = tools::bump_attempts(dir, &rel.tag);
                format!("{e} (attempt {n} of {})", tools::MAX_SMOKE_ATTEMPTS)
            }
        });
    }
    let in_use_updated = rt().updated_at.is_some();
    let dir_owned = dir.to_path_buf();
    let (tag, now_ts) = (rel.tag.clone(), now());
    let meta = tauri::async_runtime::spawn_blocking(move || {
        tools::install(&dir_owned, &staged, &tag, &hash, now_ts, in_use_updated, tools::PUBLIC_KEY)
    })
    .await
    .map_err(|e| format!("install: {e}"))?
    .map_err(|e| format!("install: {e}"))?;
    // Run it only if the installed files verify as a whole, exactly like at the next start.
    let Some(v) = tools::verified(dir, tools::PUBLIC_KEY) else {
        return Err("installed copy does not verify".into());
    };
    st.resolver.set_ytdlp_override(Some(v.exe));
    {
        let mut r = rt();
        r.in_use = Some(meta.version.clone());
        r.updated_at = Some(meta.installed_at);
        r.counter.reset();
    }
    tracing::info!(version = %meta.version, previous = ?meta.previous, "yt-dlp updated");
    let _ = st.app.emit("ytdlp:installed", YtdlpChange { version: meta.version, bad: None });
    Ok(())
}

/// `--version` prints exactly the release tag (permanent if not), then one anonymous
/// resolve works (any failure there is transient: the network, YouTube or the region may
/// be the reason).
async fn smoke_test(st: &Session, exe: &std::path::Path, tag: &str) -> Result<(), tools::Failure> {
    let v = release::installed_version_within(exe, release::FIRST_RUN_TIMEOUT)
        .await
        .map_err(|e| tools::Failure::Transient(format!("smoke test: {e}")))?;
    if v != tag {
        return Err(tools::Failure::Permanent(format!("smoke test: --version says {v}, release is {tag}")));
    }
    let id = TrackId::new(SMOKE_ID);
    match st.resolver.probe(exe, std::slice::from_ref(&id)).await.pop() {
        Some((_, Ok(_))) => Ok(()),
        Some((_, Err(e))) => Err(tools::Failure::Transient(format!("smoke test: resolve failed: {e}"))),
        None => Err(tools::Failure::Transient("smoke test: no answer".into())),
    }
}

/// One cycle: look up the recent releases; announce the newest; install the newest one that
/// is old enough when the setting is on and it was not refused, sent back or given up on.
async fn ytdlp_cycle(st: &Arc<Session>) -> YtdlpStatus {
    let _guard = busy().lock().await;
    ensure_in_use(st).await;
    let releases = match release::fetch_releases(st.yt.http()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::info!("yt-dlp check: {e}");
            return status();
        }
    };
    let Some(newest) = release::newest(&releases) else { return status() };
    let Some(in_use) = rt().in_use.clone() else { return status() };
    let auto = Prefs::load(st).ytdlp_auto_update;
    let dir = tools_dir(st);
    let (bad, reverted) = (tools::read_bad(&dir), tools::read_reverted(&dir));
    let after = tools::install_after(newest.published_at);
    {
        let mut r = rt();
        r.latest = Some(newest.tag.clone());
        r.install_after = auto.then_some(after);
        r.reverted = reverted.clone();
    }
    if !release::is_newer(&newest.tag, &in_use) {
        tracing::info!(latest = %newest.tag, %in_use, "yt-dlp is up to date");
        return status();
    }
    if !tools::wanted(&newest.tag, &in_use, &bad, &reverted) {
        tracing::info!(latest = %newest.tag, "yt-dlp release was refused or sent back before, not announcing it");
        return status();
    }
    let _ = st.app.emit(
        "ytdlp:available",
        YtdlpInfo { installed: in_use.clone(), latest: newest.tag.clone(), install_after: auto.then_some(after) },
    );
    if !auto {
        tracing::info!(latest = %newest.tag, "new yt-dlp, automatic update is off");
        return status();
    }
    let Some(cand) = tools::pick_candidate(&releases, &in_use, &bad, &reverted, now()) else {
        tracing::info!(latest = %newest.tag, after, "new yt-dlp is too young to install");
        return status();
    };
    if tools::gave_up(&dir, &cand.tag) {
        tracing::info!(version = %cand.tag, "smoke test kept failing, notice only for this version");
        return status();
    }
    if let Err(e) = install_release(st, cand).await {
        tracing::info!(version = %cand.tag, "yt-dlp update refused: {e}");
    }
    status()
}

/// A batch finished. Only a batch that ran with the updated yt-dlp that runs now counts (a
/// batch that started before an install or ran with the bundled exe because the copy
/// changed is ignored). If the updated exe keeps failing where the previous one (or the
/// bundled one) works, go back.
async fn on_batch(st: Arc<Session>, failed: Vec<TrackId>, generation: Option<u64>) {
    let Some(g) = generation else { return };
    if st.resolver.override_generation() != Some(g) {
        return;
    }
    let (Some(version), Some(bundled_version)) = ({
        let r = rt();
        (r.updated_at.and_then(|_| r.in_use.clone()), r.bundled_version.clone())
    }) else {
        return;
    };
    // A probe at a time; a batch that arrives meanwhile is not counted.
    let Ok(_guard) = busy().try_lock() else { return };
    let dir = tools_dir(&st);
    let previous_ok = if failed.is_empty() {
        false
    } else {
        // What a rollback would run: the verified previous copy, else the bundled exe.
        let back = tools::previous_verified(&dir, &bundled_version, tools::PUBLIC_KEY)
            .map(|p| p.exe)
            .unwrap_or_else(|| st.resolver.bundled_ytdlp().to_path_buf());
        let ids: Vec<TrackId> = failed.iter().take(3).cloned().collect();
        st.resolver.probe(&back, &ids).await.iter().any(|(_, r)| r.is_ok())
    };
    // The install may have changed while we probed.
    if st.resolver.override_generation() != Some(g) {
        return;
    }
    if !rt().counter.report(!failed.is_empty(), previous_ok) {
        return;
    }
    tracing::warn!(%version, "updated yt-dlp keeps failing where the previous one works, rolling back");
    // The resolver leaves the exe first; only then do the files move.
    st.resolver.set_ytdlp_override(None);
    let (d, v, b) = (dir.clone(), version.clone(), bundled_version.clone());
    let restored = tauri::async_runtime::spawn_blocking(move || tools::rollback(&d, &v, &b, tools::PUBLIC_KEY)).await;
    let (now_in_use, at) = match restored {
        Ok(Ok(Some(v))) => match tools::verified(&dir, tools::PUBLIC_KEY) {
            Some(ver) => {
                st.resolver.set_ytdlp_override(Some(ver.exe));
                (v, Some(ver.meta.installed_at))
            }
            None => (bundled_version, None),
        },
        Ok(Ok(None)) => (bundled_version, None),
        Ok(Err(e)) => {
            tracing::warn!("yt-dlp rollback: files could not be moved, keeping the bundled one: {e}");
            (bundled_version, None)
        }
        Err(e) => {
            tracing::warn!("yt-dlp rollback: {e}");
            (bundled_version, None)
        }
    };
    {
        let mut r = rt();
        r.in_use = Some(now_in_use.clone());
        r.updated_at = at;
        r.counter.reset();
    }
    let _ = st.app.emit("ytdlp:rolledback", YtdlpChange { version: now_in_use, bad: Some(version) });
}

/// The yt-dlp loop: 60 s after start, then daily, whatever the app's own update setting says.
pub fn spawn_ytdlp(st: Arc<Session>) {
    let hook_st = st.clone();
    st.resolver.set_batch_hook(move |failed, generation| {
        let (st, failed) = (hook_st.clone(), failed.to_vec());
        tauri::async_runtime::spawn(on_batch(st, failed, generation));
    });
    tauri::async_runtime::spawn(async move {
        // Which exe runs is settled at once; the lookup waits.
        ensure_in_use(&st).await;
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            ytdlp_cycle(&st).await;
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Checks by hand (and installs when it is due); no toast, the answer is the status.
#[tauri::command]
pub async fn ytdlp_check(state: State<'_, Arc<Session>>) -> CmdResult<YtdlpStatus> {
    Ok(ytdlp_cycle(state.inner()).await)
}

/// The yt-dlp in use, without touching the network.
#[tauri::command]
pub async fn ytdlp_status(state: State<'_, Arc<Session>>) -> CmdResult<YtdlpStatus> {
    ensure_in_use(state.inner()).await;
    Ok(status())
}

/// «Вернуть встроенную»: drops the updated copy and remembers the version, which then is
/// not installed again (a newer one may be).
#[tauri::command]
pub async fn ytdlp_revert(state: State<'_, Arc<Session>>) -> CmdResult<YtdlpStatus> {
    let st = state.inner();
    let _guard = busy().lock().await;
    // The resolver leaves the exe first, then the files go.
    st.resolver.set_ytdlp_override(None);
    let (version, bundled) = {
        let r = rt();
        (r.updated_at.and_then(|_| r.in_use.clone()), r.bundled_version.clone())
    };
    let dir = tools_dir(st);
    let v = version.clone();
    let res = tauri::async_runtime::spawn_blocking(move || tools::revert_to_bundled(&dir, v.as_deref())).await;
    match res {
        Ok(Ok(())) => tracing::info!(?version, "reverted to the bundled yt-dlp"),
        Ok(Err(e)) => tracing::warn!("revert: files could not be removed, the bundled one still runs: {e}"),
        Err(e) => tracing::warn!("revert: {e}"),
    }
    let reverted = tools::read_reverted(&tools_dir(st));
    {
        let mut r = rt();
        r.in_use = bundled;
        r.updated_at = None;
        r.reverted = reverted;
        r.counter.reset();
    }
    Ok(status())
}
