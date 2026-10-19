//! Demuxes and decodes one source into interleaved stereo f32 at the source rate.
//!
//! Positions are counted in *presentation* frames: encoder delay is already gone (libopus
//! pre-skip, MP4 edit list) and frame 0 is the first audible sample. Seeks are exact: a coarse
//! container seek a little earlier (pre-roll), then decoded frames before the target are dropped.
//!
//! A [`SourceOpener::Sparse`] stream is read without blocking. Decoding only starts when
//! [`READ_AHEAD`] bytes past the reader are downloaded; a read that still finds a hole (or a seek
//! into one) leaves the decoder waiting at its position, and once the bytes are there it reopens
//! the stream and seeks back exactly, since symphonia's state after a failed read is undefined.

use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;

use symphonia::core::codecs::audio::well_known::{CODEC_ID_AAC, CODEC_ID_OPUS};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::{Error as SymError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::{TimeBase, Timestamp};

use crate::{Error, Result, SourceOpener, SparseCacheFile};

/// Downloaded bytes past the reader's position before a streamed source decodes the next
/// packet: several packets plus container headers, so a decode step does not run into a hole.
/// The test fixtures are tiny, so tests use less to actually stream them.
const READ_AHEAD: u64 = if cfg!(test) { 1024 } else { 16 * 1024 };
/// Downloaded head of a streamed source before it is opened: the container header (and
/// usually the seek index) must be there, because symphonia's mkv reader treats a failed read
/// during its header scan as the end of the header.
const OPEN_BYTES: u64 = if cfg!(test) { 4096 } else { 64 * 1024 };

/// Opus needs ~80 ms of decoded audio after a reset before its output converges.
const OPUS_PREROLL_MS: u64 = 80;
/// AAC needs the previous frame for the MDCT overlap; two frames to be safe.
const AAC_PREROLL_FRAMES: u64 = 2048;
/// Consecutive undecodable packets tolerated before the source is considered broken.
const MAX_DECODE_ERRORS: u32 = 32;
/// How much of an MP4 header is scanned for the edit list.
const MP4_HEAD_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeInfo {
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u16,
    /// `"webm"`, `"m4a"`, `"ogg"`, ...
    pub container: String,
    /// `"opus"`, `"aac"`, ...
    pub codec: String,
}

/// Opens `path` and reports its format. When the container has no duration (fragmented MP4
/// without `sidx`), packet durations are summed instead.
pub fn probe(path: &Path) -> Result<ProbeInfo> {
    let hint = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let mut dec = Decoder::open(SourceOpener::File(path.to_path_buf()), hint.as_deref())?;
    let frames = match dec.duration_frames() {
        Some(f) => f,
        None => dec.count_frames()?,
    };
    Ok(ProbeInfo {
        duration_ms: frames * 1000 / dec.rate as u64,
        sample_rate: dec.rate,
        channels: dec.channels as u16,
        container: dec.container.clone(),
        codec: dec.decoder.codec_info().short_name.to_string(),
    })
}

fn codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
        registry
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Codec {
    Opus,
    Aac,
    Other,
}

pub(crate) struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: TimeBase,
    rate: u32,
    channels: usize,
    codec: Codec,
    container: String,
    /// Leading frames in packet timestamps that are not audible (MP4 edit list).
    delay: u64,
    /// Playable length from the MP4 edit list; output is cut there.
    cap: Option<u64>,
    duration: Option<u64>,
    /// Presentation frame of the next emitted frame.
    pos: u64,
    /// Drop everything before this frame (seek target, MP4 delay).
    target: Option<u64>,
    /// Until the next emitted frame, positions come from packet timestamps (after a seek).
    from_pts: bool,
    /// libopus pre-skip the adapter will still trim from the first packet it decodes.
    preskip_pending: u64,
    lookahead: Option<Packet>,
    errors: u32,
    scratch: Vec<f32>,
    /// The demuxer reached the end; symphonia readers cannot seek back from there, so the next
    /// seek reopens the source.
    drained: bool,
    /// A seek went past the last packet.
    past_end: bool,
    /// A streamed source waits for the download at this presentation frame: it is reopened and
    /// sought there once the bytes are in.
    resume: Option<u64>,
    opener: SourceOpener,
    hint: Option<String>,
}

/// Whether a streamed source can be opened without running into a hole: its head is there, and
/// so is whatever an earlier attempt stopped at. A missing head is asked for.
pub(crate) fn can_open(file: &SparseCacheFile) -> bool {
    if !file.ready_at(0, OPEN_BYTES) {
        file.want(0);
        return false;
    }
    file.ready(READ_AHEAD)
}

/// Maps "the bytes are not downloaded yet" (a non-blocking read's `WouldBlock`, wherever
/// symphonia wrapped it) to [`Error::Buffering`].
fn pending(e: Error) -> Error {
    let would_block = match &e {
        Error::Io(io) | Error::Decode(SymError::IoError(io)) => io.kind() == ErrorKind::WouldBlock,
        _ => false,
    };
    if would_block { Error::Buffering } else { e }
}

impl Decoder {
    /// Opens the source. A streamed one whose header is not downloaded yet fails with
    /// [`Error::Buffering`] (see [`can_open`]).
    pub fn open(opener: SourceOpener, hint_ext: Option<&str>) -> Result<Self> {
        Self::try_open(opener, hint_ext).map_err(pending)
    }

    fn try_open(opener: SourceOpener, hint_ext: Option<&str>) -> Result<Self> {
        let mut src = opener.clone().open()?;
        let edit = mp4_edit(&mut src)?;
        let mss = MediaSourceStream::new(src, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = hint_ext {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(Error::NoTrack)?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or(Error::NoTrack)?;
        let rate = params
            .sample_rate
            .filter(|r| *r > 0)
            .ok_or(Error::NoTrack)?;
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count())
            .unwrap_or(2)
            .max(1);
        let codec = match params.codec {
            CODEC_ID_OPUS => Codec::Opus,
            CODEC_ID_AAC => Codec::Aac,
            _ => Codec::Other,
        };
        let preskip = match codec {
            Codec::Opus => params.extra_data.as_deref().map(opus_preskip).unwrap_or(0),
            _ => 0,
        };
        let decoder = codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;
        let time_base = track
            .time_base
            .or_else(|| TimeBase::try_from_recip(rate))
            .ok_or(Error::NoTrack)?;
        let track_id = track.id;

        let container = match format.format_info().short_name {
            "matroska" if hint_ext == Some("mkv") => "mkv",
            "matroska" => "webm",
            "isomp4" => "m4a",
            other => other,
        }
        .to_string();
        let (delay, cap) = match edit {
            Some(e) => (e.delay(rate), e.length(rate)),
            None => (0, None),
        };
        let duration = cap.or_else(|| {
            let frames = |ts: u64, tb: TimeBase| ts_to_frames(ts as i64, tb, rate).max(0) as u64;
            track
                .duration
                .filter(|d| d.get() > 0)
                .map(|d| frames(d.get(), time_base))
                .or_else(|| {
                    let info = format.media_info();
                    let tb = info.time_base?;
                    info.duration
                        .filter(|d| d.get() > 0)
                        .map(|d| frames(d.get(), tb))
                })
                .map(|f| f.saturating_sub(delay))
        });

        Ok(Self {
            format,
            decoder,
            track_id,
            time_base,
            rate,
            channels,
            codec,
            container,
            delay,
            cap,
            duration,
            pos: 0,
            // MP4 delay is not trimmed by anyone else: start in timestamp mode to drop it.
            target: (delay > 0).then_some(0),
            from_pts: delay > 0,
            preskip_pending: preskip * rate as u64 / 48_000,
            lookahead: None,
            errors: 0,
            scratch: Vec::new(),
            drained: false,
            past_end: false,
            resume: None,
            opener,
            hint: hint_ext.map(str::to_string),
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn position(&self) -> u64 {
        self.target.unwrap_or(self.pos)
    }

    /// Playable length in frames, when the container states it.
    pub fn duration_frames(&self) -> Option<u64> {
        self.duration
    }

    /// Decodes the next packet and appends its stereo frames to `out` (possibly none while
    /// skipping to a seek target). Returns `false` once the stream has ended, and
    /// [`Error::Buffering`] (nothing consumed) while a streamed source waits for its bytes.
    pub fn next_chunk(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        self.resume()?;
        if !self.ready() {
            return Err(Error::Buffering);
        }
        loop {
            let capped = self
                .cap
                .is_some_and(|c| self.pos >= c && self.target.is_none());
            if self.past_end || capped {
                return Ok(false);
            }
            let packet = match self.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(false),
                Err(e) => {
                    let e = pending(e);
                    if matches!(e, Error::Buffering) {
                        self.wait_at(self.position());
                    }
                    return Err(e);
                }
            };
            let last = self.lookahead.is_none();
            let buf = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                Err(SymError::DecodeError(msg)) => {
                    self.errors += 1;
                    if self.errors > MAX_DECODE_ERRORS {
                        return Err(SymError::DecodeError(msg).into());
                    }
                    tracing::debug!(msg, "skipping undecodable packet");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            self.errors = 0;
            let frames = buf.frames();
            if buf.spec().rate() != self.rate {
                self.rate = buf.spec().rate();
            }
            let channels = buf.spec().channels().count().max(1);
            self.channels = channels;
            self.scratch.resize(frames * channels, 0.0);
            buf.copy_to_slice_interleaved(&mut self.scratch[..]);

            let preskip = std::mem::take(&mut self.preskip_pending) as i64;
            let start = match self.from_pts {
                true => {
                    ts_to_frames(packet.pts.get(), self.time_base, self.rate) + preskip
                        - self.delay as i64
                }
                false => self.pos as i64,
            };
            let mut end = start + frames as i64;
            if last {
                // The container's duration of the final packet excludes encoder padding
                // (WebM DiscardPadding shows up as a shorter block duration).
                let dur = ts_to_frames(packet.dur.get() as i64, self.time_base, self.rate);
                if dur > 0 && dur < frames as i64 {
                    end = start + dur;
                }
            }
            if let Some(cap) = self.cap {
                end = end.min(cap as i64);
            }
            let from = self.target.map_or(start, |t| start.max(t as i64));
            if end <= from {
                if last {
                    return Ok(false);
                }
                // Counting from `pos` (a seek near the start reopens instead of seeking), the
                // next packet starts where this one ended.
                self.pos = end.max(0) as u64;
                continue;
            }
            let (a, b) = ((from - start) as usize, (end - start) as usize);
            push_stereo(&self.scratch[a * channels..b * channels], channels, out);
            self.pos = end as u64;
            self.target = None;
            self.from_pts = false;
            return Ok(true);
        }
    }

    /// Moves to `ms` exactly; the next emitted frame is the one at `ms`. On a streamed source
    /// whose bytes there are missing the seek completes in a later [`Decoder::next_chunk`].
    pub fn seek(&mut self, ms: u64) -> Result<()> {
        let mut target = ms * self.rate as u64 / 1000;
        if let Some(d) = self.duration {
            target = target.min(d);
        }
        let res = match self.resume {
            // Parked after a failed read: the reader's state is undefined.
            Some(_) => self.reopen().and_then(|()| self.seek_frame(target)),
            None => self.seek_frame(target),
        };
        match res.map_err(pending) {
            Err(Error::Buffering) => {
                self.wait_at(target);
                Ok(())
            }
            res => res,
        }
    }

    /// Whether the next packet can be read without running into a hole of a streamed source.
    fn ready(&self) -> bool {
        match &self.opener {
            SourceOpener::Sparse(file) => file.ready(READ_AHEAD),
            _ => true,
        }
    }

    /// Parks a streamed source at presentation frame `at` until its bytes are downloaded.
    fn wait_at(&mut self, at: u64) {
        self.resume = Some(at);
        self.target = Some(at);
        self.pos = at;
        self.lookahead = None;
        self.past_end = false;
    }

    /// Picks a parked stream up again: reopens it (the reader may be anywhere after a failed
    /// read) and seeks exactly where it stopped.
    fn resume(&mut self) -> Result<()> {
        let Some(at) = self.resume else {
            return Ok(());
        };
        if !self.ready() {
            return Err(Error::Buffering);
        }
        match self
            .reopen()
            .and_then(|()| self.seek_frame(at))
            .map_err(pending)
        {
            Err(Error::Buffering) => {
                self.wait_at(at);
                Err(Error::Buffering)
            }
            Err(e) => Err(e),
            Ok(()) => {
                self.resume = None;
                Ok(())
            }
        }
    }

    fn seek_frame(&mut self, target: u64) -> Result<()> {
        let rate = self.rate as u64;
        let preroll = match self.codec {
            Codec::Opus => OPUS_PREROLL_MS * rate / 1000,
            Codec::Aac => AAC_PREROLL_FRAMES,
            Codec::Other => 0,
        };
        let media = (target + self.delay).saturating_sub(preroll);
        let ts = Timestamp::new(frames_to_ts(media, self.time_base, self.rate));
        if media == 0 {
            // Near the start a fresh reader is exact (demuxers may skip the first packet, whose
            // timestamp is negative) and cheap.
            self.reopen()?;
            self.target = self.target.max(Some(target));
            return Ok(());
        }
        if self.drained {
            self.reopen()?;
        }
        let to = |track_id| SeekTo::Timestamp { ts, track_id };
        let res = match self.format.seek(SeekMode::Accurate, to(self.track_id)) {
            Err(SymError::SeekError(SeekErrorKind::OutOfRange)) => {
                Err(SymError::SeekError(SeekErrorKind::OutOfRange))
            }
            // A reader left in a bad state by an earlier error: start over once.
            Err(_) => {
                self.reopen()?;
                self.format.seek(SeekMode::Accurate, to(self.track_id))
            }
            ok => ok,
        };
        self.past_end = match res {
            Ok(_) => false,
            // Past the last packet: the stream simply ends (and the reader is at its end).
            Err(SymError::SeekError(SeekErrorKind::OutOfRange)) => {
                self.drained = true;
                true
            }
            Err(e) => return Err(e.into()),
        };
        self.lookahead = None;
        self.decoder.reset();
        self.target = Some(target);
        self.from_pts = true;
        self.pos = target;
        Ok(())
    }

    /// Fills in a missing duration by counting packets with a second reader. Local files only:
    /// the end of a stream still being downloaded may not be there yet.
    pub fn measure(&mut self) {
        if self.duration.is_some() || !matches!(self.opener, SourceOpener::File(_)) {
            return;
        }
        match Decoder::open(self.opener.clone(), self.hint.as_deref())
            .and_then(|mut d| d.count_frames())
        {
            Ok(frames) => self.duration = Some(frames),
            Err(e) => tracing::debug!("cannot measure the duration: {e}"),
        }
    }

    fn reopen(&mut self) -> Result<()> {
        let measured = self.duration;
        *self = Decoder::open(self.opener.clone(), self.hint.as_deref())?;
        self.duration = self.duration.or(measured);
        Ok(())
    }

    /// Reads (without decoding) the rest of the stream and sums packet durations.
    fn count_frames(&mut self) -> Result<u64> {
        let mut total = 0i64;
        while let Some(p) = self.next_packet()? {
            total += ts_to_frames(p.dur.get() as i64, self.time_base, self.rate);
        }
        Ok((total.max(0) as u64).saturating_sub(self.delay))
    }

    fn next_packet(&mut self) -> Result<Option<Packet>> {
        let current = match self.lookahead.take() {
            Some(p) => p,
            None => match self.read_packet()? {
                Some(p) => p,
                None => return Ok(None),
            },
        };
        self.lookahead = self.read_packet()?;
        Ok(Some(current))
    }

    fn read_packet(&mut self) -> Result<Option<Packet>> {
        loop {
            match self.format.next_packet() {
                Ok(Some(p)) if p.track_id == self.track_id => return Ok(Some(p)),
                Ok(Some(_)) => {}
                Ok(None) | Err(SymError::ResetRequired) => {
                    self.drained = true;
                    return Ok(None);
                }
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.drained = true;
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// Appends `src` (interleaved, `channels` wide) as stereo: mono is duplicated, extra channels
/// beyond front left/right are dropped.
fn push_stereo(src: &[f32], channels: usize, out: &mut Vec<f32>) {
    match channels {
        2 => out.extend_from_slice(src),
        1 => out.extend(src.iter().flat_map(|s| [*s, *s])),
        n => out.extend(src.chunks_exact(n).flat_map(|f| [f[0], f[1]])),
    }
}

fn ts_to_frames(ts: i64, tb: TimeBase, rate: u32) -> i64 {
    let num = ts as i128 * tb.numer.get() as i128 * rate as i128;
    let den = tb.denom.get() as i128;
    (num.signum() * ((num.abs() + den / 2) / den)) as i64
}

fn frames_to_ts(frames: u64, tb: TimeBase, rate: u32) -> i64 {
    let den = tb.numer.get() as u128 * rate as u128;
    (frames as u128 * tb.denom.get() as u128 / den) as i64
}

/// Pre-skip from an `OpusHead` (48 kHz frames).
fn opus_preskip(head: &[u8]) -> u64 {
    match head {
        [
            b'O',
            b'p',
            b'u',
            b's',
            b'H',
            b'e',
            b'a',
            b'd',
            _,
            _,
            lo,
            hi,
            ..,
        ] => u16::from_le_bytes([*lo, *hi]) as u64,
        _ => 0,
    }
}

/// First non-empty entry of an audio track's MP4 edit list. Symphonia does not apply edit
/// lists, so AAC priming (and the padded tail) would otherwise be played.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mp4Edit {
    media_time: u64,
    media_scale: u32,
    segment_duration: u64,
    movie_scale: u32,
}

impl Mp4Edit {
    fn delay(&self, rate: u32) -> u64 {
        self.media_time * rate as u64 / self.media_scale.max(1) as u64
    }

    fn length(&self, rate: u32) -> Option<u64> {
        (self.segment_duration > 0 && self.movie_scale > 0)
            .then(|| self.segment_duration * rate as u64 / self.movie_scale as u64)
    }
}

/// Reads the head of `src` and looks for an edit list when it is an MP4. Leaves `src` at 0.
/// A stream still downloading may stop short of [`MP4_HEAD_BYTES`]: that is enough once the
/// `moov` box is complete.
fn mp4_edit(src: &mut Box<dyn MediaSource>) -> Result<Option<Mp4Edit>> {
    let mut head = Vec::new();
    if let Err(e) = src.by_ref().take(MP4_HEAD_BYTES).read_to_end(&mut head) {
        let enough = e.kind() == ErrorKind::WouldBlock
            && head.len() >= 8
            && (head[4..8] != *b"ftyp" || child(&head, b"moov").is_some());
        if !enough {
            return Err(e.into());
        }
    }
    src.seek(SeekFrom::Start(0))?;
    if head.get(4..8) != Some(b"ftyp") {
        return Ok(None);
    }
    Ok(find_mp4_edit(&head))
}

fn find_mp4_edit(file: &[u8]) -> Option<Mp4Edit> {
    let moov = child(file, b"moov")?;
    let movie_scale = timescale(child(moov, b"mvhd")?)?;
    for trak in children(moov).filter(|(t, _)| t == b"trak").map(|(_, b)| b) {
        let Some(mdia) = child(trak, b"mdia") else {
            continue;
        };
        if child(mdia, b"hdlr").and_then(|h| h.get(8..12)) != Some(b"soun") {
            continue;
        }
        let media_scale = timescale(child(mdia, b"mdhd")?)?;
        let elst = child(child(trak, b"edts")?, b"elst")?;
        let (version, count) = (
            elst[0],
            u32::from_be_bytes(elst.get(4..8)?.try_into().ok()?),
        );
        let mut entries = elst.get(8..)?;
        for _ in 0..count {
            let (duration, media_time, len) = if version == 1 {
                (
                    be_u64(entries.get(0..8)?),
                    be_u64(entries.get(8..16)?) as i64,
                    20,
                )
            } else {
                let t = i32::from_be_bytes(entries.get(4..8)?.try_into().ok()?);
                (be_u32(entries.get(0..4)?) as u64, t as i64, 12)
            };
            if media_time >= 0 {
                return Some(Mp4Edit {
                    media_time: media_time as u64,
                    media_scale,
                    segment_duration: duration,
                    movie_scale,
                });
            }
            entries = entries.get(len..)?;
        }
        return None;
    }
    None
}

/// Timescale of an `mvhd`/`mdhd` payload.
fn timescale(full_box: &[u8]) -> Option<u32> {
    let at = if full_box.first()? == &1 { 20 } else { 12 };
    full_box.get(at..at + 4).map(be_u32)
}

fn child<'a>(parent: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(parent).find(|(t, _)| t == kind).map(|(_, b)| b)
}

/// Iterates `(type, payload)` of the boxes laid out in `buf`; stops at a truncated box.
fn children(mut buf: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let size = be_u32(buf.get(0..4)?) as u64;
        let kind: [u8; 4] = buf.get(4..8)?.try_into().ok()?;
        let (header, size) = match size {
            0 => (8, buf.len() as u64),
            1 => (16, be_u64(buf.get(8..16)?)),
            n => (8, n),
        };
        let size = usize::try_from(size)
            .ok()
            .filter(|s| *s >= header && *s <= buf.len())?;
        let payload = &buf[header..size];
        buf = &buf[size..];
        Some((kind, payload))
    })
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn be_u64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    pub fn open(name: &str) -> Decoder {
        let path = fixture(name);
        let ext = path.extension().unwrap().to_str().unwrap().to_string();
        Decoder::open(SourceOpener::File(path), Some(&ext)).unwrap()
    }

    pub fn decode_all(dec: &mut Decoder) -> Vec<f32> {
        let mut out = Vec::new();
        while dec.next_chunk(&mut out).unwrap() {}
        out
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    #[test]
    fn decodes_opus_webm() {
        let mut dec = open("sine_opus.webm");
        assert_eq!(
            (dec.rate, dec.codec, dec.container.as_str()),
            (48_000, Codec::Opus, "webm")
        );
        let pcm = decode_all(&mut dec);
        let ms = pcm.len() as u64 / 2 * 1000 / 48_000;
        assert!(ms.abs_diff(2000) <= 30, "{ms} ms");
        assert!(rms(&pcm) > 0.05, "silent: {}", rms(&pcm));
    }

    #[test]
    fn decodes_fragmented_aac() {
        let mut dec = open("sine_aac.m4a");
        assert_eq!(
            (dec.rate, dec.codec, dec.container.as_str()),
            (44_100, Codec::Aac, "m4a")
        );
        let pcm = decode_all(&mut dec);
        let ms = pcm.len() as u64 / 2 * 1000 / 44_100;
        assert!(ms.abs_diff(2000) <= 30, "{ms} ms");
        assert!(rms(&pcm) > 0.05, "silent: {}", rms(&pcm));
    }

    #[test]
    fn probe_reports_format() {
        let info = probe(&fixture("sine_opus.webm")).unwrap();
        assert_eq!((info.sample_rate, info.channels), (48_000, 2));
        assert_eq!(
            (info.container.as_str(), info.codec.as_str()),
            ("webm", "opus")
        );
        assert!(info.duration_ms.abs_diff(2000) <= 30, "{info:?}");

        let info = probe(&fixture("sine_aac.m4a")).unwrap();
        assert_eq!((info.sample_rate, info.container.as_str()), (44_100, "m4a"));
        assert!(info.duration_ms.abs_diff(2000) <= 30, "{info:?}");
    }

    /// How far (seconds, within ±half a period) the audio in `pcm` is from `sin(2π·440·t)` at
    /// `t0`: the sine fixtures start at phase 0 at t = 0.
    fn phase_error(pcm: &[f32], rate: u32, t0: f64) -> f64 {
        let amp = pcm.iter().take(4096).fold(0f32, |m, s| m.max(s.abs())) as f64;
        let period = 1.0 / 440.0;
        (0..400)
            .map(|k| period * (k as f64 / 400.0 - 0.5))
            .map(|d| {
                let err: f64 = (0..512)
                    .map(|i| {
                        let t = t0 + d + i as f64 / rate as f64;
                        let want = amp * (std::f64::consts::TAU * 440.0 * t).sin();
                        (pcm[i * 2] as f64 - want).powi(2)
                    })
                    .sum();
                (d, err)
            })
            .fold((0.0, f64::MAX), |a, b| if b.1 < a.1 { b } else { a })
            .0
    }

    /// Seeking to 1.25 s must emit exactly the tail a full decode emits after 1.25 s (absolute
    /// position, independent of codec padding). MP4 timestamps are sample-exact, so the AAC tail
    /// must also be in phase with the source; WebM timestamps are whole milliseconds and
    /// symphonia truncates the 6.5 ms CodecDelay, so Opus lands up to ~1.5 ms early.
    fn check_seek(name: &str, rate: u32, tolerance_ms: f64, phase_tolerance_ms: Option<f64>) {
        let total = decode_all(&mut open(name)).len() / 2;

        let mut dec = open(name);
        dec.seek(1250).unwrap();
        let mut tail = Vec::new();
        while tail.len() < 4096 && dec.next_chunk(&mut tail).unwrap() {}
        assert_eq!(
            dec.position(),
            (1250 * rate / 1000) as u64 + tail.len() as u64 / 2
        );
        if let Some(tol) = phase_tolerance_ms {
            let phase = phase_error(&tail, rate, 1.25) * 1000.0;
            assert!(phase.abs() <= tol, "{name}: phase off by {phase:.3} ms");
        }

        while dec.next_chunk(&mut tail).unwrap() {}
        let landed_ms = (total - tail.len() / 2) as f64 * 1000.0 / rate as f64;
        assert!(
            (landed_ms - 1250.0).abs() <= tolerance_ms,
            "{name}: landed at {landed_ms:.2} ms"
        );
    }

    #[test]
    fn seek_is_exact_opus() {
        check_seek("sine_opus.webm", 48_000, 2.0, None);
    }

    #[test]
    fn seek_is_exact_aac() {
        check_seek("sine_aac.m4a", 44_100, 0.1, Some(0.05));
    }

    #[test]
    fn seeks_back_after_the_end() {
        for name in ["sine_aac.m4a", "sine_opus.webm"] {
            let mut dec = open(name);
            let full = decode_all(&mut dec);
            dec.seek(0).unwrap();
            assert_eq!(decode_all(&mut dec).len(), full.len(), "{name}");
            dec.seek(60_000).unwrap();
            assert!(
                !dec.next_chunk(&mut Vec::new()).unwrap(),
                "{name}: past the end"
            );
            dec.seek(1000).unwrap();
            assert!(decode_all(&mut dec).len() / 2 > 40_000, "{name}");
        }
    }

    #[test]
    fn parses_edit_list() {
        // moov > [mvhd(v0, timescale 1000), trak > [edts > elst(v0: 2000, 1024), mdia > [hdlr soun, mdhd(v0, 44100)]]]
        fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(payload);
            v
        }
        let mut mvhd = vec![0u8; 12];
        mvhd.extend(1000u32.to_be_bytes());
        mvhd.extend([0u8; 4]);
        let mut mdhd = vec![0u8; 12];
        mdhd.extend(44_100u32.to_be_bytes());
        mdhd.extend([0u8; 4]);
        let mut hdlr = vec![0u8; 8];
        hdlr.extend(b"soun");
        let mut elst = vec![0, 0, 0, 0];
        elst.extend(1u32.to_be_bytes());
        elst.extend(2000u32.to_be_bytes());
        elst.extend(1024i32.to_be_bytes());
        elst.extend([0, 1, 0, 0]);
        let mdia = [bx(b"hdlr", &hdlr), bx(b"mdhd", &mdhd)].concat();
        let trak = [bx(b"edts", &bx(b"elst", &elst)), bx(b"mdia", &mdia)].concat();
        let moov = [bx(b"mvhd", &mvhd), bx(b"trak", &trak)].concat();
        let file = [bx(b"ftyp", b"M4A \0\0\0\0"), bx(b"moov", &moov)].concat();

        let edit = find_mp4_edit(&file).unwrap();
        assert_eq!(edit.delay(44_100), 1024);
        assert_eq!(edit.length(44_100), Some(88_200));
        assert_eq!(find_mp4_edit(&file[..30]), None);
    }

    #[test]
    fn timestamp_conversion_rounds() {
        let ms = TimeBase::try_new(1, 1000).unwrap();
        assert_eq!(ts_to_frames(14, ms, 48_000), 672);
        assert_eq!(ts_to_frames(-7, ms, 48_000), -336);
        assert_eq!(frames_to_ts(48_000, ms, 48_000), 1000);
        assert_eq!(opus_preskip(b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0"), 312);
    }
}
