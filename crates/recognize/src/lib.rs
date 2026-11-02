//! «Распознать трек» (specs 002): sound becomes a Shazam signature here on the PC and Shazam's
//! recognition endpoint names the song. No API keys: only the signature (spectral peaks) leaves
//! the machine.
//!
//! The signature algorithm, its binary format and the request are ported from SongRec
//! (https://github.com/marin-m/SongRec, `src/core/fingerprinting`), © Marin Moulinier and
//! contributors, GPL-3.0-or-later, through the phone version (mixtape-android, specs 011).

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use realfft::RealFftPlanner;
use realfft::num_complex::Complex;
use serde::Serialize;
use serde_json::Value;
use std::time::{Duration, SystemTime};
use ts_rs::TS;

/// Shazam's sample rate: the peaks are found on 16 kHz mono.
pub const RATE: u32 = 16_000;
/// Shorter than this is not worth a request.
pub const MIN_SAMPLES: usize = 2 * RATE as usize;
/// The signature always spans 12 s, padded with silence, as SongRec does.
pub const SIGNATURE_SAMPLES: usize = 12 * RATE as usize;

// ---------- Signature ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peak {
    /// Number of the 128-sample step (8 ms) the peak is at.
    pub pass: u32,
    pub magnitude: u16,
    /// FFT bin × 64, corrected between bins.
    pub bin: u16,
}

/// A decoded Shazam signature: peaks in four frequency bands
/// (250–520, 520–1450, 1450–3500, 3500–5500 Hz).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub sample_rate: u32,
    pub samples: u32,
    pub bands: [Vec<Peak>; 4],
}

const URI_PREFIX: &str = "data:audio/vnd.shazam.sig;base64,";
const MAGIC1: u32 = 0xcafe_2580;
const MAGIC2: u32 = 0x9411_9c00;
const BAND_TAG: u32 = 0x6003_0040;
const HEADER: usize = 48;

fn rate_id(rate: u32) -> Option<u32> {
    Some(match rate {
        8000 => 1,
        11025 => 2,
        16000 => 3,
        32000 => 4,
        44100 => 5,
        48000 => 6,
        _ => return None,
    })
}

#[cfg(test)]
fn rate_of(id: u32) -> Option<u32> {
    Some(match id {
        1 => 8000,
        2 => 11025,
        3 => 16000,
        4 => 32000,
        5 => 44100,
        6 => 48000,
        _ => return None,
    })
}

impl Signature {
    /// The signature of 16 kHz mono PCM (SongRec's `SignatureGenerator`).
    pub fn from_pcm(pcm: &[i16]) -> Self {
        let mut samples = pcm.to_vec();
        if samples.len() < SIGNATURE_SAMPLES {
            samples.resize(SIGNATURE_SAMPLES, 0);
        }
        let mut g = Generator::new(samples.len() as u32);
        for chunk in samples.chunks_exact(128) {
            g.step(chunk);
        }
        g.signature
    }

    /// Recorded length for the request, ms.
    pub fn sample_ms(&self) -> u32 {
        (self.samples as f32 / self.sample_rate as f32 * 1000.0) as u32
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::with_capacity(HEADER + 8 + self.bands.iter().map(|b| b.len() * 5 + 16).sum::<usize>());
        let put = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        put(&mut out, MAGIC1);
        put(&mut out, 0); // CRC-32 of everything after the first 8 bytes, written last
        put(&mut out, 0); // size minus the header, written last
        put(&mut out, MAGIC2);
        for _ in 0..3 {
            put(&mut out, 0);
        }
        put(&mut out, rate_id(self.sample_rate).unwrap_or(3) << 27);
        put(&mut out, 0);
        put(&mut out, 0);
        put(&mut out, self.samples + (self.sample_rate as f32 * 0.24) as u32);
        put(&mut out, (15 << 19) + 0x40000);
        put(&mut out, 0x4000_0000);
        put(&mut out, 0); // size minus the header again
        for (band, peaks) in self.bands.iter().enumerate() {
            if peaks.is_empty() {
                continue;
            }
            let mut body = Vec::with_capacity(peaks.len() * 5);
            let mut pass = 0u32;
            for p in peaks {
                if p.pass - pass >= 255 {
                    body.push(0xff);
                    body.extend_from_slice(&p.pass.to_le_bytes());
                    pass = p.pass;
                }
                body.push((p.pass - pass) as u8);
                body.extend_from_slice(&p.magnitude.to_le_bytes());
                body.extend_from_slice(&p.bin.to_le_bytes());
                pass = p.pass;
            }
            put(&mut out, BAND_TAG + band as u32);
            put(&mut out, body.len() as u32);
            let pad = (4 - body.len() % 4) % 4;
            out.extend_from_slice(&body);
            out.extend(std::iter::repeat_n(0u8, pad));
        }
        let size = (out.len() - HEADER) as u32;
        out[8..12].copy_from_slice(&size.to_le_bytes());
        out[HEADER + 4..HEADER + 8].copy_from_slice(&size.to_le_bytes());
        let crc = crc32fast::hash(&out[8..]);
        out[4..8].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// Reads a signature back (tests: round trip of the encoding).
    #[cfg(test)]
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        let u32_at = |i: usize| -> Result<u32, String> {
            data.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "truncated".to_string())
        };
        if data.len() < HEADER + 8 {
            return Err("too short".into());
        }
        if u32_at(0)? != MAGIC1 || u32_at(12)? != MAGIC2 {
            return Err("bad magic".into());
        }
        if u32_at(8)? as usize != data.len() - HEADER || u32_at(HEADER + 4)? as usize != data.len() - HEADER {
            return Err("bad size".into());
        }
        if u32_at(4)? != crc32fast::hash(&data[8..]) {
            return Err("bad crc".into());
        }
        let sample_rate = rate_of(u32_at(28)? >> 27).ok_or("bad sample rate")?;
        let samples = u32_at(40)?.wrapping_sub((sample_rate as f32 * 0.24) as u32);
        if u32_at(HEADER)? != 0x4000_0000 {
            return Err("bad first chunk".into());
        }
        let mut bands: [Vec<Peak>; 4] = Default::default();
        let mut at = HEADER + 8;
        while at < data.len() {
            let band = u32_at(at)?.wrapping_sub(BAND_TAG) as usize;
            let size = u32_at(at + 4)? as usize;
            at += 8;
            let body = data.get(at..at + size).ok_or("truncated band")?;
            let peaks = bands.get_mut(band).ok_or("bad band")?;
            let mut i = 0;
            let mut pass = 0u32;
            while i < body.len() {
                let step = body[i];
                i += 1;
                if step == 0xff {
                    let b = body.get(i..i + 4).ok_or("truncated pass")?;
                    pass = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    i += 4;
                    continue;
                }
                pass += step as u32;
                let b = body.get(i..i + 4).ok_or("truncated peak")?;
                peaks.push(Peak { pass, magnitude: u16::from_le_bytes([b[0], b[1]]), bin: u16::from_le_bytes([b[2], b[3]]) });
                i += 4;
            }
            at += size + (4 - size % 4) % 4;
        }
        Ok(Self { sample_rate, samples, bands })
    }

    pub fn uri(&self) -> String {
        format!("{URI_PREFIX}{}", BASE64_STANDARD.encode(self.encode()))
    }

    #[cfg(test)]
    pub fn from_uri(uri: &str) -> Result<Self, String> {
        let b64 = uri.strip_prefix(URI_PREFIX).ok_or("not a signature uri")?;
        Self::decode(&BASE64_STANDARD.decode(b64).map_err(|e| e.to_string())?)
    }
}

const FFT: usize = 2048;
const BINS: usize = FFT / 2 + 1;

/// SongRec's `SignatureGenerator`: a 2048-sample Hann-windowed FFT every 128 samples, peaks
/// spread in frequency and time, then local maxima kept as peaks.
struct Generator {
    ring: Vec<i16>,
    ring_at: usize,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    /// Ring of the last 256 spectra.
    ffts: Vec<[f32; BINS]>,
    ffts_at: u8,
    spread: Vec<[f32; BINS]>,
    spread_at: u8,
    spread_done: u32,
    signature: Signature,
}

impl Generator {
    fn new(samples: u32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT);
        // numpy.hanning(2050)[1:-1], as SongRec's table.
        let window = (0..FFT).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * (i + 1) as f64 / 2049.0).cos()) as f32).collect();
        Self {
            ring: vec![0; FFT],
            ring_at: 0,
            window,
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            fft,
            ffts: vec![[0.0; BINS]; 256],
            ffts_at: 0,
            spread: vec![[0.0; BINS]; 256],
            spread_at: 0,
            spread_done: 0,
            signature: Signature { sample_rate: RATE, samples, bands: Default::default() },
        }
    }

    fn step(&mut self, chunk: &[i16]) {
        self.do_fft(chunk);
        self.spread_peaks();
        self.spread_done += 1;
        if self.spread_done >= 46 {
            self.find_peaks();
        }
    }

    fn do_fft(&mut self, chunk: &[i16]) {
        self.ring[self.ring_at..self.ring_at + 128].copy_from_slice(chunk);
        self.ring_at = (self.ring_at + 128) & (FFT - 1);
        // Oldest first, windowed.
        for (i, w) in self.window.iter().enumerate() {
            self.input[i] = self.ring[(i + self.ring_at) & (FFT - 1)] as f32 * w;
        }
        self.fft.process(&mut self.input, &mut self.output).expect("fft sizes are fixed");
        let out = &mut self.ffts[self.ffts_at as usize];
        for (o, c) in out.iter_mut().zip(&self.output) {
            *o = ((c.re * c.re + c.im * c.im) / (1 << 17) as f32).max(1e-10);
        }
        self.ffts_at = self.ffts_at.wrapping_add(1);
    }

    fn spread_peaks(&mut self) {
        let mut cur = self.ffts[self.ffts_at.wrapping_sub(1) as usize];
        // In frequency.
        for p in 0..=BINS - 3 {
            cur[p] = cur[p].max(cur[p + 1]).max(cur[p + 2]);
        }
        self.spread[self.spread_at as usize] = cur;
        // In time.
        for back in [1u8, 3, 6] {
            let former = &mut self.spread[self.spread_at.wrapping_sub(back) as usize];
            for (f, c) in former.iter_mut().zip(&cur) {
                *f = f.max(*c);
            }
        }
        self.spread_at = self.spread_at.wrapping_add(1);
    }

    fn find_peaks(&mut self) {
        let fft46 = &self.ffts[self.ffts_at.wrapping_sub(46) as usize];
        let spread49 = &self.spread[self.spread_at.wrapping_sub(49) as usize];
        let mag = |v: f32| v.ln().max(1.0 / 64.0) * 1477.3 + 6144.0;
        for bin in 10..=1014usize {
            let v = fft46[bin];
            if v < 1.0 / 64.0 || v < spread49[bin - 1] {
                continue;
            }
            // A local maximum in frequency…
            let mut around = 0f32;
            for off in [-10i32, -7, -4, -3, 1, 2, 5, 8] {
                around = around.max(spread49[(bin as i32 + off) as usize]);
            }
            if v <= around {
                continue;
            }
            // …and in time.
            for off in [-53i32, -45, 165, 172, 179, 186, 193, 200, 214, 221, 228, 235, 242, 249] {
                let other = &self.spread[((self.spread_at as i32 + off) & 255) as usize];
                around = around.max(other[bin - 1]);
            }
            if v <= around {
                continue;
            }
            let pass = self.spread_done - 46;
            let m = mag(v);
            let before = mag(fft46[bin - 1]);
            let after = mag(fft46[bin + 1]);
            let var1 = m * 2.0 - before - after;
            if var1 <= 0.0 {
                continue;
            }
            let var2 = (after - before) * 32.0 / var1;
            let corrected = ((bin as i32 * 64) + var2 as i32) as u16;
            let hz = corrected as f32 * (RATE as f32 / 2.0 / 1024.0 / 64.0);
            let band = match hz as i32 {
                250..=519 => 0,
                520..=1449 => 1,
                1450..=3499 => 2,
                3500..=5500 => 3,
                _ => continue,
            };
            self.signature.bands[band].push(Peak { pass, magnitude: m as u16, bin: corrected });
        }
    }
}

/// Mono PCM at `rate` to 16 kHz: each output sample averages the input it covers (a crude
/// low-pass, enough for the 44.1 kHz fallback).
pub fn to_16k(pcm: &[i16], rate: u32) -> Vec<i16> {
    if rate == RATE || rate == 0 {
        return pcm.to_vec();
    }
    let ratio = rate as f64 / RATE as f64;
    let n = (pcm.len() as f64 / ratio) as usize;
    (0..n)
        .map(|i| {
            let a = (i as f64 * ratio) as usize;
            let b = (((i + 1) as f64 * ratio) as usize).clamp(a + 1, pcm.len());
            let sum: i64 = pcm[a..b].iter().map(|&s| s as i64).sum();
            (sum / (b - a) as i64) as i16
        })
        .collect()
}

// ---------- Shazam ----------

/// What Shazam named.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecognizedSong {
    pub title: String,
    pub artist: String,
    #[ts(optional)]
    pub album: Option<String>,
    #[ts(optional)]
    pub cover_url: Option<String>,
}

#[derive(Debug)]
pub enum ShazamError {
    /// No answer: offline, or the server is down.
    Network(String),
    /// 429: too many requests from this address.
    Limited,
    Other(String),
}

/// From SongRec's list: Shazam's Android app identifies itself with Dalvik.
const USER_AGENTS: [&str; 4] = [
    "Dalvik/2.1.0 (Linux; U; Android 5.0.2; VS980 4G Build/LRX22G)",
    "Dalvik/2.1.0 (Linux; U; Android 5.1.1; SM-P905V Build/LMY47X)",
    "Dalvik/2.1.0 (Linux; U; Android 6.0.1; SM-G920F Build/MMB29K)",
    "Dalvik/2.1.0 (Linux; U; Android 7.0; SM-G930F Build/NRD90M)",
];

/// The request body SongRec sends.
fn request_body(sig: &Signature, now_ms: u64) -> Value {
    serde_json::json!({
        "geolocation": { "altitude": 300, "latitude": 45, "longitude": 2 },
        "signature": {
            "samplems": sig.sample_ms(),
            "timestamp": now_ms as u32,
            "uri": sig.uri(),
        },
        "timestamp": now_ms as u32,
        "timezone": "Europe/Paris",
    })
}

fn request_url() -> String {
    let a = uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase();
    let b = uuid::Uuid::new_v4().hyphenated().to_string();
    format!(
        "https://amp.shazam.com/discovery/v5/en/US/android/-/tag/{a}/{b}?sync=true&webv3=true&sampling=true&connected=&shazamapiversion=v3&sharehub=true&video=v3"
    )
}

/// Asks Shazam; `None` when it knows no match.
pub async fn shazam(http: &reqwest::Client, sig: &Signature) -> Result<Option<RecognizedSong>, ShazamError> {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let ua = USER_AGENTS[(now as usize / 7) % USER_AGENTS.len()];
    let resp = http
        .post(request_url())
        .header(reqwest::header::USER_AGENT, ua)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("Content-Language", "en_US")
        .body(request_body(sig, now).to_string())
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| ShazamError::Network(e.to_string()))?;
    let status = resp.status();
    if status.as_u16() == 429 {
        return Err(ShazamError::Limited);
    }
    let bytes = resp.bytes().await.map_err(|e| ShazamError::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(ShazamError::Other(format!("shazam: HTTP {status}")));
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| ShazamError::Other(format!("shazam: {e}")))?;
    Ok(parse(&v))
}

/// The song in Shazam's answer; `None` when `matches` is empty.
pub fn parse(v: &Value) -> Option<RecognizedSong> {
    if v.get("matches")?.as_array()?.is_empty() {
        return None;
    }
    let track = v.get("track")?;
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let title = text(track.get("title"))?;
    let artist = text(track.get("subtitle")).unwrap_or_default();
    let images = track.get("images");
    let cover_url = text(images.and_then(|i| i.get("coverarthq"))).or_else(|| text(images.and_then(|i| i.get("coverart"))));
    let album = track
        .get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s.get("type").and_then(Value::as_str) == Some("SONG"))
        .filter_map(|s| s.get("metadata")?.as_array())
        .flatten()
        .find(|m| m.get("title").and_then(Value::as_str) == Some("Album"))
        .and_then(|m| text(m.get("text")));
    Some(RecognizedSong { title, artist, album, cover_url })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chord that moves every half second, `secs` long, at 16 kHz.
    fn tones(secs: f32) -> Vec<i16> {
        let n = (secs * RATE as f32) as usize;
        let notes = [440.0f32, 660.0, 880.0, 1320.0, 2000.0, 3000.0, 4400.0];
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let k = (t * 2.0) as usize;
                let a = notes[k % notes.len()];
                let b = notes[(k + 3) % notes.len()];
                let s = (2.0 * std::f32::consts::PI * a * t).sin() + 0.6 * (2.0 * std::f32::consts::PI * b * t).sin();
                (s * 9000.0) as i16
            })
            .collect()
    }

    /// Beeps of a sine: 200 ms with a smooth (Hann) envelope every 500 ms, so the peaks
    /// are onsets of the tone itself rather than clicks.
    fn sine(hz: f32, secs: f32) -> Vec<i16> {
        let n = (secs * RATE as f32) as usize;
        let beep = RATE as usize / 5;
        let period = RATE as usize / 2;
        (0..n)
            .map(|i| {
                let k = i % period;
                if k >= beep {
                    return 0;
                }
                let env = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / beep as f32).cos();
                ((2.0 * std::f32::consts::PI * hz * i as f32 / RATE as f32).sin() * env * 12000.0) as i16
            })
            .collect()
    }

    #[test]
    fn sine_peaks_land_in_its_band() {
        let sig = Signature::from_pcm(&sine(1000.0, 6.0));
        assert_eq!(sig.sample_rate, RATE);
        // Padded to 12 s like SongRec.
        assert_eq!(sig.samples as usize, SIGNATURE_SAMPLES);
        assert_eq!(sig.sample_ms(), 12_000);
        // Loud peaks are the beeps themselves, one per beep, on 1 kHz (520–1450 Hz band);
        // the rest are faint ones at the noise floor.
        let hz = |p: &Peak| p.bin as f32 * (RATE as f32 / 2.0 / 1024.0 / 64.0);
        let loud: Vec<&Peak> = sig.bands.iter().flatten().filter(|p| p.magnitude > 20_000).collect();
        assert_eq!(loud.len(), 12, "one per beep");
        assert!(loud.iter().all(|p| (hz(p) - 1000.0).abs() < 5.0 && sig.bands[1].contains(p)));
        // A beep each 500 ms = 62.5 steps of 8 ms.
        assert!(loud.windows(2).all(|w| (62..=63).contains(&(w[1].pass - w[0].pass))));
    }

    #[test]
    fn signature_is_deterministic() {
        let pcm = tones(8.0);
        let a = Signature::from_pcm(&pcm);
        let b = Signature::from_pcm(&pcm);
        assert_eq!(a, b);
        assert_eq!(a.encode(), b.encode());
        assert!(a.bands.iter().filter(|b| !b.is_empty()).count() >= 3);
    }

    #[test]
    fn encoding_round_trips() {
        let sig = Signature::from_pcm(&tones(10.0));
        let bytes = sig.encode();
        assert_eq!(&bytes[0..4], &MAGIC1.to_le_bytes());
        assert_eq!(bytes.len() % 4, 0);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), crc32fast::hash(&bytes[8..]));
        assert_eq!(Signature::decode(&bytes).unwrap(), sig);
        let uri = sig.uri();
        assert!(uri.starts_with(URI_PREFIX));
        assert_eq!(Signature::from_uri(&uri).unwrap(), sig);
    }

    #[test]
    fn long_gaps_use_the_escape() {
        let sig = Signature {
            sample_rate: RATE,
            samples: 16_000,
            bands: [
                vec![Peak { pass: 3, magnitude: 7000, bin: 1200 }, Peak { pass: 700, magnitude: 6500, bin: 1300 }],
                vec![],
                vec![Peak { pass: 0, magnitude: 1, bin: 2 }],
                vec![],
            ],
        };
        let bytes = sig.encode();
        assert_eq!(Signature::decode(&bytes).unwrap(), sig);
        let mut broken = bytes.clone();
        broken[60] ^= 1;
        assert_eq!(Signature::decode(&broken).unwrap_err(), "bad crc");
    }

    #[test]
    fn resamples_to_16k() {
        let pcm: Vec<i16> = (0..44_100).map(|i| (i % 100) as i16).collect();
        let out = to_16k(&pcm, 44_100);
        assert_eq!(out.len(), 16_000);
        assert_eq!(to_16k(&pcm[..10], RATE), pcm[..10].to_vec());
    }

    #[test]
    fn request_body_has_signature() {
        let sig = Signature::from_pcm(&sine(700.0, 4.0));
        let body = request_body(&sig, 1_700_000_000_123);
        assert_eq!(body["signature"]["samplems"], 12_000);
        assert!(body["signature"]["uri"].as_str().unwrap().starts_with(URI_PREFIX));
        assert_eq!(body["timezone"], "Europe/Paris");
        let url = request_url();
        assert!(url.starts_with("https://amp.shazam.com/discovery/v5/en/US/android/-/tag/"));
    }

    const FOUND: &str = r#"{
      "matches": [{"id": "1234", "offset": 31.2, "timeskew": 0.0001, "frequencyskew": 0.0}],
      "location": {"accuracy": 0.01},
      "timestamp": 1700000000,
      "timezone": "Europe/Paris",
      "track": {
        "layout": "5",
        "type": "MUSIC",
        "key": "5933917",
        "title": "Blinding Lights",
        "subtitle": "The Weeknd",
        "images": {
          "background": "https://is1-ssl.mzstatic.com/image/thumb/bg/800x800cc.jpg",
          "coverart": "https://is1-ssl.mzstatic.com/image/thumb/cover/400x400cc.jpg",
          "coverarthq": "https://is1-ssl.mzstatic.com/image/thumb/cover/400x400cc_hq.jpg"
        },
        "url": "https://www.shazam.com/track/5933917/blinding-lights",
        "sections": [
          {
            "type": "SONG",
            "metadata": [
              {"title": "Album", "text": "After Hours"},
              {"title": "Label", "text": "Republic Records"},
              {"title": "Released", "text": "2019"}
            ]
          },
          {"type": "LYRICS", "text": ["..."]}
        ]
      },
      "tagid": "ABCD"
    }"#;

    const NOT_FOUND: &str = r#"{"matches": [], "timestamp": 1700000000, "timezone": "Europe/Paris", "tagid": "ABCD", "retryms": 4000}"#;

    #[test]
    fn parses_a_match() {
        let song = parse(&serde_json::from_str(FOUND).unwrap()).unwrap();
        assert_eq!(
            song,
            RecognizedSong {
                title: "Blinding Lights".into(),
                artist: "The Weeknd".into(),
                album: Some("After Hours".into()),
                cover_url: Some("https://is1-ssl.mzstatic.com/image/thumb/cover/400x400cc_hq.jpg".into()),
            }
        );
    }

    #[test]
    fn parses_no_match() {
        assert_eq!(parse(&serde_json::from_str(NOT_FOUND).unwrap()), None);
        // A match list without a track is treated as nothing found too.
        assert_eq!(parse(&serde_json::json!({"matches": [{"id": "1"}]})), None);
        // No album section, only the plain cover.
        let v = serde_json::json!({"matches": [{}], "track": {"title": "X", "subtitle": "Y", "images": {"coverart": "c"}}});
        assert_eq!(parse(&v), Some(RecognizedSong { title: "X".into(), artist: "Y".into(), album: None, cover_url: Some("c".into()) }));
    }

    /// The real endpoint with a synthetic signal: Shazam answers and the answer parses
    /// (no match expected). `MIXTAPE_RECOGNIZE_PCM` may name a raw s16le 16 kHz mono file
    /// of real music instead (`MIXTAPE_RECOGNIZE_SECS` of its middle), then the song is printed.
    /// `cargo test --lib recognize::tests::live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "goes to amp.shazam.com"]
    async fn live() {
        let pcm = match std::env::var("MIXTAPE_RECOGNIZE_PCM") {
            Ok(path) => {
                let bytes = std::fs::read(path).unwrap();
                let all: Vec<i16> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                // `MIXTAPE_RECOGNIZE_SECS` (default 12) from the middle, like the 4 s and 8 s tries.
                let secs: usize = std::env::var("MIXTAPE_RECOGNIZE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
                let half = secs * RATE as usize / 2;
                let mid = all.len() / 2;
                all[mid.saturating_sub(half)..(mid + half).min(all.len())].to_vec()
            }
            Err(_) => tones(10.0),
        };
        let sig = Signature::from_pcm(&pcm);
        let http = reqwest::Client::builder().build().unwrap();
        match shazam(&http, &sig).await {
            Ok(song) => println!("shazam answered: {song:?}"),
            Err(e) => panic!("shazam failed: {e:?}"),
        }
    }
}
