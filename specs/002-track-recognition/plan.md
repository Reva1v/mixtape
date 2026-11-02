# Plan: Распознавание трека на ПК

Spec: [spec.md](spec.md). Port of `mixtape-android/src-tauri/src/recognize.rs` and `Recognize.svelte`.

## Interfaces

- `crates/recognize` (new): `Signature::from_pcm`, `shazam(http, &sig)`, `parse`, `to_16k`, `RecognizedSong`. Copied from the
  phone's module, without its Kotlin parts. Reason for a new crate: it has no app state and its deps stay out of the rest.
- `mixtape_audio::capture`: `Capture::start(CaptureSource)`, `samples()`, `rate()`, `level()`; drop frees the device. Computer
  sound is cpal's WASAPI loopback, an input stream on the default output device. cpal is already a dependency.
- `session::commands::recognize`: `recognize_start(source)`, `recognize_cancel`, `recognize_level`, `recognize_settings`.
- `ui/src/lib/ui/Recognize.svelte`, a button on the search screen right of the field. The source lives in `localStorage`.

## Rules

- Shazam is asked at 4, 8 and 12 s; each start or cancel moves the run on, and an older run stops.
- The device is held only by the current run, and freed on a match, on cancel and at the end.
- `level` is polled by the UI at 10 Hz, within the constitution's limit, and only while listening.
- The session's HTTP client adds no cookies by itself: they are set per request, so Shazam gets none.

## Tests

- Ported: signature determinism, encode round trip, peaks in the right band, Shazam answer parsing.
- New: mono mix and clamping, level scale, a denied microphone in any language.
- Live, by hand: a known song on this PC through each source.
