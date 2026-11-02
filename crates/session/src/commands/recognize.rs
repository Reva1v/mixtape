//! «Распознать трек» (specs 002): a few seconds of sound become a Shazam signature here, Shazam
//! names the song and YouTube Music gives the track to play. The device is held only while a
//! run is listening; sound is neither stored nor sent, only the signature leaves the PC.

use super::{CmdResult, blocking};
use crate::state::Session;
use mixtape_audio::capture::{Capture, CaptureError, CaptureSource};
use mixtape_core::{AppError, ErrorKind, Track};
use mixtape_recognize::{
    MIN_SAMPLES, RATE, RecognizedSong, SIGNATURE_SAMPLES, ShazamError, Signature, shazam, to_16k,
};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;
use ts_rs::TS;

type St<'a> = State<'a, Arc<Session>>;

/// When Shazam is asked, s after the recording started; the first match ends it.
const ATTEMPTS: [u64; 3] = [4, 8, 12];
/// How often a run looks whether it was cancelled.
const TICK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RecognizeStatus {
    Found,
    NotFound,
    Cancelled,
    /// Windows does not let the app use the microphone.
    Denied,
    /// No microphone or no output device.
    NoDevice,
    /// No network.
    Offline,
    /// Shazam refused: too many requests.
    Limited,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecognizeResult {
    pub status: RecognizeStatus,
    #[ts(optional)]
    pub song: Option<RecognizedSong>,
    /// The song on YouTube Music, stored in the library (none if not found there).
    #[ts(optional)]
    pub track: Option<Track>,
}

impl RecognizeResult {
    fn of(status: RecognizeStatus) -> Self {
        Self {
            status,
            song: None,
            track: None,
        }
    }
}

/// The run in progress: the latest start owns the device, and any other is over.
#[derive(Default)]
pub struct Recognition {
    /// Moved on by each start and each cancel.
    run: AtomicU64,
    capture: Mutex<Option<Arc<Capture>>>,
}

impl Recognition {
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Arc<Capture>>> {
        self.capture.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn begin(&self) -> u64 {
        self.run.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn current(&self, run: u64) -> bool {
        self.run.load(Ordering::SeqCst) == run
    }

    /// Stops listening and any older run with it. Off the UI thread: the device is let go here.
    fn cancel(&self) {
        self.begin();
        self.slot().take();
    }

    /// Hands the device over to `run`, unless another run has taken its place meanwhile.
    fn hold(&self, run: u64, capture: Arc<Capture>) -> bool {
        let mut slot = self.slot();
        if !self.current(run) {
            return false;
        }
        *slot = Some(capture);
        true
    }

    /// `run` is done with the device.
    fn release(&self, run: u64) {
        let mut slot = self.slot();
        if self.current(run) {
            slot.take();
        }
    }
}

/// Listens and names the song: Shazam is asked at 4, 8 and 12 s of sound, the first match ends
/// the recording. Resolves `cancelled` once `recognize_cancel` or a new start ran.
#[tauri::command]
pub async fn recognize_start(state: St<'_>, source: CaptureSource) -> CmdResult<RecognizeResult> {
    let st = state.inner().clone();
    let run = st.recognition.begin();
    let started = blocking(move || Ok(Capture::start(source))).await?;
    let capture = match started {
        Ok(c) => Arc::new(c),
        Err(CaptureError::Denied) => return Ok(RecognizeResult::of(RecognizeStatus::Denied)),
        Err(CaptureError::NoDevice) => return Ok(RecognizeResult::of(RecognizeStatus::NoDevice)),
        Err(CaptureError::Other(e)) => return Err(AppError::new(ErrorKind::Internal, e)),
    };
    if !st.recognition.hold(run, capture.clone()) {
        return Ok(RecognizeResult::of(RecognizeStatus::Cancelled));
    }
    let out = listen(&st, run, &capture).await;
    st.recognition.release(run);
    out
}

async fn listen(st: &Arc<Session>, run: u64, capture: &Arc<Capture>) -> CmdResult<RecognizeResult> {
    let begun = Instant::now();
    let cancelled = || Ok(RecognizeResult::of(RecognizeStatus::Cancelled));
    for (n, at) in ATTEMPTS.iter().enumerate() {
        let last = n + 1 == ATTEMPTS.len();
        let due = begun + Duration::from_secs(*at);
        while Instant::now() < due {
            if !st.recognition.current(run) {
                return cancelled();
            }
            tokio::time::sleep(TICK.min(due.saturating_duration_since(Instant::now()))).await;
        }
        let rate = capture.rate();
        let source = capture.clone();
        let signature = blocking(move || {
            let pcm = to_16k(&source.samples(), rate);
            if pcm.len() < MIN_SAMPLES {
                return Ok(None);
            }
            Ok(Some(Signature::from_pcm(
                &pcm[pcm.len().saturating_sub(SIGNATURE_SAMPLES)..],
            )))
        })
        .await?;
        if !st.recognition.current(run) {
            return cancelled();
        }
        // Loopback delivers nothing while the computer is silent.
        let Some(signature) = signature else { continue };
        tracing::debug!(
            seconds = signature.samples / RATE,
            "recognize: asking Shazam"
        );
        match shazam(st.yt.http(), &signature).await {
            Ok(Some(song)) => {
                if !st.recognition.current(run) {
                    return cancelled();
                }
                // Found: the device goes at once, the search below may take a while.
                st.recognition.release(run);
                let track = find_track(st, &song).await;
                return Ok(RecognizeResult {
                    status: RecognizeStatus::Found,
                    song: Some(song),
                    track,
                });
            }
            Ok(None) if last => {}
            Ok(None) => continue,
            Err(ShazamError::Limited) => return Ok(RecognizeResult::of(RecognizeStatus::Limited)),
            Err(ShazamError::Network(e)) => {
                tracing::warn!(error = %e, "recognize: Shazam unreachable");
                return Ok(RecognizeResult::of(RecognizeStatus::Offline));
            }
            Err(ShazamError::Other(e)) => return Err(AppError::new(ErrorKind::Internal, e)),
        }
    }
    Ok(RecognizeResult::of(RecognizeStatus::NotFound))
}

/// The song on YouTube Music; without an account Shazam's answer is what there is.
async fn find_track(st: &Arc<Session>, song: &RecognizedSong) -> Option<Track> {
    if !crate::account::usable(st) {
        return None;
    }
    let query = format!("{} {}", song.artist, song.title);
    match super::catalog::first_track(st, &query, &song.title).await {
        Ok(track) => track,
        Err(e) => {
            tracing::warn!(error = %e.message, "recognize: YouTube Music search failed");
            None
        }
    }
}

/// Stops listening at once; the running `recognize_start` resolves `cancelled`.
#[tauri::command]
pub async fn recognize_cancel(state: St<'_>) -> CmdResult<()> {
    let st = state.inner().clone();
    blocking(move || {
        st.recognition.cancel();
        Ok(())
    })
    .await
}

/// How loud the source is while listening, 0..1 (the screen's live bars).
#[tauri::command]
pub fn recognize_level(state: St<'_>) -> f32 {
    state.recognition.slot().as_ref().map_or(0.0, |c| c.level())
}

/// Windows' page that allows apps to use the microphone.
#[tauri::command]
pub async fn recognize_settings() -> CmdResult<()> {
    blocking(|| {
        std::process::Command::new("explorer")
            .arg("ms-settings:privacy-microphone")
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::new(ErrorKind::Internal, e.to_string()))
    })
    .await
}
