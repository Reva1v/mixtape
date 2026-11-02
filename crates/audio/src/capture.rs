//! Records sound for song recognition: the microphone, or what the computer is playing
//! (WASAPI loopback: an input stream on the output device). Nothing is written anywhere; the
//! samples live here until the recording is dropped, which also frees the device.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use serde::Deserialize;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;
use ts_rs::TS;

/// The most kept, s: recognition looks at the last 12 s of a 12 s recording.
const MAX_SECONDS: usize = 14;
/// With no sound for this long the level reads as zero: loopback delivers nothing in silence.
const LEVEL_FRESH_MS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CaptureSource {
    /// What the computer is playing, on the default output device.
    Computer,
    Microphone,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    #[error("no audio device")]
    NoDevice,
    #[error("access to the device is denied")]
    Denied,
    #[error("{0}")]
    Other(String),
}

/// What opening a stream failed with. Windows refuses a denied microphone with E_ACCESSDENIED,
/// and its message comes in the system's language.
fn classify(message: String) -> CaptureError {
    let lower = message.to_lowercase();
    if lower.contains("0x80070005")
        || lower.contains("denied")
        || lower.contains("отказано")
        || lower.contains("відмовлено")
    {
        CaptureError::Denied
    } else {
        CaptureError::Other(message)
    }
}

/// One channel of `data`, averaged, as 16 bits.
fn to_mono<T: SizedSample>(data: &[T], channels: usize, out: &mut Vec<i16>, max: usize)
where
    f32: FromSample<T>,
{
    for frame in data.chunks_exact(channels.max(1)) {
        if out.len() >= max {
            return;
        }
        let sum: f32 = frame.iter().map(|&s| f32::from_sample_(s)).sum();
        let v = sum / frame.len() as f32;
        out.push((v.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16);
    }
}

/// How loud a block is, 0..1, on a scale that makes quiet music visible.
fn level_of(block: &[i16]) -> f32 {
    if block.is_empty() {
        return 0.0;
    }
    let sum: f64 = block.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum / block.len() as f64).sqrt() / f64::from(i16::MAX);
    ((rms * 5.0).min(1.0).sqrt()) as f32
}

#[derive(Default)]
struct Shared {
    samples: Mutex<Vec<i16>>,
    level: AtomicU32,
    /// When the level was last set, ms after the start.
    level_at: AtomicU64,
}

/// A running recording; dropping it stops the stream and frees the device.
pub struct Capture {
    shared: Arc<Shared>,
    begun: Instant,
    rate: u32,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Opens the device and starts recording. The stream lives on a thread of its own because
    /// it may not leave the thread that made it.
    pub fn start(source: CaptureSource) -> Result<Self, CaptureError> {
        let shared = Arc::new(Shared::default());
        let begun = Instant::now();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("recognize-capture".into())
            .spawn(move || match open(source, thread_shared, begun) {
                Ok((stream, rate)) => {
                    let _ = ready_tx.send(Ok(rate));
                    // Until the owner stops the recording or goes away.
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| CaptureError::Other(e.to_string()))?;
        let rate = ready_rx
            .recv()
            .map_err(|e| CaptureError::Other(e.to_string()))??;
        Ok(Self {
            shared,
            begun,
            rate,
            stop: Some(stop_tx),
            thread: Some(thread),
        })
    }

    /// The device's sample rate; the samples are mono at this rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Everything recorded so far.
    pub fn samples(&self) -> Vec<i16> {
        self.shared
            .samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// How loud it is right now, 0..1.
    pub fn level(&self) -> f32 {
        let now = self.begun.elapsed().as_millis() as u64;
        if now.saturating_sub(self.shared.level_at.load(Ordering::Relaxed)) > LEVEL_FRESH_MS {
            return 0.0;
        }
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn open(
    source: CaptureSource,
    shared: Arc<Shared>,
    begun: Instant,
) -> Result<(cpal::Stream, u32), CaptureError> {
    let host = cpal::default_host();
    let (device, config) = match source {
        CaptureSource::Computer => {
            let device = host.default_output_device().ok_or(CaptureError::NoDevice)?;
            let config = device
                .default_output_config()
                .map_err(|e| classify(e.to_string()))?;
            (device, config)
        }
        CaptureSource::Microphone => {
            let device = host.default_input_device().ok_or(CaptureError::NoDevice)?;
            let config = device
                .default_input_config()
                .map_err(|e| classify(e.to_string()))?;
            (device, config)
        }
    };
    let format = config.sample_format();
    let config = config.config();
    let (channels, rate) = (usize::from(config.channels), config.sample_rate);
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, channels, rate, shared, begun),
        SampleFormat::I16 => build::<i16>(&device, config, channels, rate, shared, begun),
        SampleFormat::I32 => build::<i32>(&device, config, channels, rate, shared, begun),
        SampleFormat::U16 => build::<u16>(&device, config, channels, rate, shared, begun),
        other => Err(CaptureError::Other(format!(
            "unsupported sample format {other}"
        ))),
    }?;
    stream.play().map_err(|e| classify(e.to_string()))?;
    tracing::info!(?source, rate, channels, "recording for recognition");
    Ok((stream, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    rate: u32,
    shared: Arc<Shared>,
    begun: Instant,
) -> Result<cpal::Stream, CaptureError>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let max = rate as usize * MAX_SECONDS;
    let mut block = Vec::new();
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                block.clear();
                to_mono(data, channels, &mut block, usize::MAX);
                shared
                    .level
                    .store(level_of(&block).to_bits(), Ordering::Relaxed);
                shared
                    .level_at
                    .store(begun.elapsed().as_millis() as u64, Ordering::Relaxed);
                let mut samples = shared.samples.lock().unwrap_or_else(|e| e.into_inner());
                let room = max.saturating_sub(samples.len());
                samples.extend_from_slice(&block[..block.len().min(room)]);
            },
            |e| tracing::warn!(error = %e, "recording stream failed"),
            None,
        )
        .map_err(|e| classify(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opens the real output device and listens for a moment; nothing is played or sent.
    /// `cargo test -p mixtape-audio --lib capture::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore = "uses the real sound device"]
    fn live_loopback_opens_and_lets_go() {
        for _ in 0..2 {
            let c = Capture::start(CaptureSource::Computer).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1500));
            println!(
                "rate {} samples {} level {}",
                c.rate(),
                c.samples().len(),
                c.level()
            );
            assert!(c.rate() >= 8000);
        }
    }

    #[test]
    fn frames_average_into_one_channel() {
        let mut out = Vec::new();
        to_mono(
            &[0.5f32, 0.0, -1.0, -1.0, 2.0, 2.0],
            2,
            &mut out,
            usize::MAX,
        );
        assert_eq!(out, [8191, -32767, 32767], "clamped, not wrapped");
    }

    #[test]
    fn integer_samples_scale_too() {
        let mut out = Vec::new();
        to_mono(&[i16::MAX, i16::MAX], 2, &mut out, usize::MAX);
        assert!(out[0] > 32_700);
    }

    #[test]
    fn level_grows_with_the_sound() {
        assert_eq!(level_of(&[]), 0.0);
        assert_eq!(level_of(&[0; 100]), 0.0);
        let quiet = level_of(&[300; 100]);
        let loud = level_of(&[20_000; 100]);
        assert!(quiet > 0.0 && quiet < loud && loud <= 1.0, "{quiet} {loud}");
    }

    #[test]
    fn a_denied_microphone_is_told_apart_in_any_language() {
        for message in [
            "Access is denied. (0x80070005)",
            "Отказано в доступе.",
            "Відмовлено в доступі",
        ] {
            assert_eq!(classify(message.into()), CaptureError::Denied, "{message}");
        }
        assert_eq!(
            classify("device busy".into()),
            CaptureError::Other("device busy".into())
        );
    }
}
