//! Listening bench for «Умные переходы»: analyses the tracks in `music/`, renders every pair of
//! `tracks.json` through the engine's real transition path (smart, and the plain crossfade for
//! comparison) into `out/clips/*.wav`, and writes `out/index.html` to listen to them.
//!
//! Nothing is played: the engine runs without an output device.
//!
//!   cargo run -p mixbench --profile mixbench -- [--pre 20] [--post 15] [--xfade 6] [--only 3]

use std::fmt::Write as _;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use mixtape_audio::analysis::{TempoDetail, tempo_detail_file};
use mixtape_audio::bench::{self, BenchPlan, Rendered};
use mixtape_audio::{Key, Source, TrackAnalysis, analyze_file, gain_for, measure_loudness, probe};
use serde::Deserialize;

#[derive(Deserialize)]
struct Spec {
    tracks: Vec<TrackSpec>,
    pairs: Vec<(String, String)>,
}

#[derive(Deserialize, Clone)]
struct TrackSpec {
    id: String,
    file: String,
    title: String,
    artist: String,
    genre: String,
    expected_bpm: Option<f64>,
    page: String,
    licence: String,
}

struct Track {
    spec: TrackSpec,
    path: PathBuf,
    duration_ms: u64,
    analysis: TrackAnalysis,
    detail: Option<TempoDetail>,
    analysis_ms: u128,
    lufs: Option<f64>,
    gain_db: f32,
}

impl Track {
    fn source(&self) -> Source {
        let mut s = Source::file(&self.spec.id, &self.path, self.gain_db);
        s.analysis = Some(Arc::new(self.analysis.clone()));
        s
    }

    /// Detected tempo against the expected one; for a rejected beat, its best grid before the
    /// steadiness thresholds.
    fn tempo_verdict(&self) -> String {
        let accepted = self.analysis.bpm > 0.0;
        let bpm = match (accepted, self.detail) {
            (true, _) => self.analysis.bpm as f64,
            (false, Some(d)) => d.bpm,
            (false, None) => return "нет доли".into(),
        };
        let what = match self.spec.expected_bpm {
            None => "эталона нет".to_string(),
            Some(e) => {
                let r = bpm / e;
                if (r - 1.0).abs() <= 0.02 {
                    "ok".into()
                } else if (r - 0.5).abs() <= 0.02 {
                    "половинный темп".into()
                } else if (r - 2.0).abs() <= 0.04 {
                    "двойной темп".into()
                } else if (r - 2.0 / 3.0).abs() <= 0.02 || (r - 1.5).abs() <= 0.03 {
                    format!("ошибка ×{r:.2} (3/2)")
                } else {
                    format!("мимо ({:+.1}%)", (r - 1.0) * 100.0)
                }
            }
        };
        if accepted {
            what
        } else {
            format!("отброшен; сетка: {what}")
        }
    }
}

struct Opts {
    pre_ms: u64,
    post_ms: u64,
    xfade_ms: u32,
    only: Option<usize>,
}

fn main() {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let opts = parse_args();
    let spec: Spec =
        serde_json::from_str(&fs::read_to_string(here.join("tracks.json")).expect("tracks.json"))
            .expect("tracks.json is valid");
    let music = here.join("music");
    let out = here.join("out");
    fs::create_dir_all(out.join("clips")).expect("out/clips");

    println!("== Анализ треков");
    let mut tracks = Vec::new();
    for t in &spec.tracks {
        let path = music.join(&t.file);
        if !path.exists() {
            eprintln!("нет файла {} — запустите fetch-music.ps1", path.display());
            continue;
        }
        tracks.push(load_track(t.clone(), path));
    }
    print_track_table(&tracks);

    println!("\n== Пары");
    let mut cards = Vec::new();
    for (i, (a, b)) in spec.pairs.iter().enumerate() {
        if opts.only.is_some_and(|o| o != i + 1) {
            continue;
        }
        let find = |id: &str| tracks.iter().find(|t| t.spec.id == id);
        let (Some(ta), Some(tb)) = (find(a), find(b)) else {
            eprintln!("пара {a} → {b}: трека нет, пропуск");
            continue;
        };
        cards.push(render_pair(i + 1, ta, tb, &opts, &out));
    }
    print_pair_table(&cards);

    let html = report(&tracks, &cards, &opts);
    fs::write(out.join("index.html"), html).expect("write index.html");
    println!("\nОтчёт: {}", out.join("index.html").display());
}

fn parse_args() -> Opts {
    let mut o = Opts {
        pre_ms: 20_000,
        post_ms: 15_000,
        xfade_ms: 6_000,
        only: None,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let v = || -> f64 {
            args.get(i + 1)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| panic!("{} needs a number", args[i]))
        };
        match args[i].as_str() {
            "--pre" => o.pre_ms = (v() * 1000.0) as u64,
            "--post" => o.post_ms = (v() * 1000.0) as u64,
            "--xfade" => o.xfade_ms = (v() * 1000.0) as u32,
            "--only" => o.only = Some(v() as usize),
            other => panic!("unknown argument {other}"),
        }
        i += 2;
    }
    o
}

fn load_track(spec: TrackSpec, path: PathBuf) -> Track {
    let duration_ms = probe(&path).map(|p| p.duration_ms).unwrap_or(0);
    let t0 = Instant::now();
    let analysis = analyze_file(&path).unwrap_or_else(|e| {
        eprintln!("{}: analysis failed: {e}", spec.id);
        TrackAnalysis::empty()
    });
    let analysis_ms = t0.elapsed().as_millis();
    let detail = tempo_detail_file(&path).ok().flatten();
    let loud = measure_loudness(&path).ok();
    let gain_db = gain_for(loud.map(|l| l.lufs), loud.map(|l| l.true_peak));
    Track {
        spec,
        path,
        duration_ms,
        analysis,
        detail,
        analysis_ms,
        lufs: loud.map(|l| l.lufs),
        gain_db,
    }
}

const NOTES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

fn key_name(k: Option<Key>) -> String {
    match k {
        Some(k) => format!(
            "{}{} ({}{})",
            NOTES[k.tonic() as usize],
            if k.minor() { "m" } else { "" },
            k.camelot(),
            if k.minor() { "A" } else { "B" }
        ),
        None => "—".into(),
    }
}

fn secs(ms: u64) -> String {
    format!("{}:{:04.1}", ms / 60_000, (ms % 60_000) as f64 / 1000.0)
}

fn print_track_table(tracks: &[Track]) {
    println!(
        "{:<18} {:<16} {:>6} {:>7} {:>7} {:<34} {:>5} {:>5} {:<12} {:>7} {:>7} {:>6} {:>6}",
        "id",
        "genre",
        "exp",
        "bpm",
        "raw",
        "verdict",
        "per",
        "str",
        "key",
        "intro",
        "outro",
        "dur",
        "ms"
    );
    for t in tracks {
        let a = &t.analysis;
        println!(
            "{:<18} {:<16} {:>6} {:>7.2} {:>7} {:<34} {:>5} {:>5} {:<12} {:>7} {:>7} {:>6} {:>6}",
            t.spec.id,
            t.spec.genre,
            t.spec.expected_bpm.map_or("?".into(), |b| format!("{b}")),
            a.bpm,
            t.detail.map_or("—".into(), |d| format!("{:.1}", d.bpm)),
            t.tempo_verdict(),
            t.detail
                .map_or("—".into(), |d| format!("{:.2}", d.periodicity)),
            t.detail
                .map_or("—".into(), |d| format!("{:.2}", d.strength)),
            key_name(a.key),
            secs(a.intro_end_ms),
            secs(a.outro_start_ms),
            secs(t.duration_ms),
            t.analysis_ms,
        );
    }
}

/// Why the engine did what it did for `a → b`.
struct Diagnosis {
    mode: Mode,
    /// Tempo ratio after half/double matching (`b` played this much faster), and the multiple.
    ratio: Option<(f64, f64)>,
    keys_ok: Option<bool>,
    reason: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Synced,
    Unsynced,
    Fallback,
}

fn diagnose(a: &Track, b: &Track, plan: Option<&BenchPlan>) -> Diagnosis {
    let (aa, ba) = (&a.analysis, &b.analysis);
    let ratio = match (aa.beat_period_ms(), ba.beat_period_ms()) {
        (Some(pa), Some(pb)) => [0.5, 1.0, 2.0]
            .into_iter()
            .map(|m| ((pb * m) / pa, m))
            .min_by(|x, y| (x.0 - 1.0).abs().total_cmp(&(y.0 - 1.0).abs())),
        _ => None,
    };
    let keys_ok = match (aa.key, ba.key) {
        (Some(x), Some(y)) => Some(x.compatible(y)),
        _ => None,
    };
    let synced = ratio.is_some_and(|(s, _)| (s - 1.0).abs() <= 0.06);
    let mut why = Vec::new();
    if aa.bpm <= 0.0 {
        why.push("у A нет устойчивой доли".to_string());
    }
    if ba.bpm <= 0.0 {
        why.push("у B нет устойчивой доли".to_string());
    }
    if let Some((s, _)) = ratio
        && !synced
    {
        why.push(format!(
            "темпы не сводятся в ±6 % (нужно {:+.1} %)",
            (1.0 / s - 1.0) * 100.0
        ));
    }
    let mode = match plan {
        None => Mode::Fallback,
        Some(p) if synced && (p.speed - ratio.map_or(1.0, |r| r.0)).abs() < 1e-9 => Mode::Synced,
        Some(_) if synced => Mode::Synced,
        Some(_) => Mode::Unsynced,
    };
    let reason = match mode {
        Mode::Synced => {
            let mut r = "доли сведены".to_string();
            if keys_ok == Some(false) {
                r += "; тональности конфликтуют → короче";
            }
            r
        }
        Mode::Unsynced => format!("смешение 4 с без сведения: {}", why.join("; ")),
        Mode::Fallback => {
            if !aa.usable() || !ba.usable() {
                "анализ непригоден (тишина/ошибка)".into()
            } else {
                "план не уместился: ни одна длина в тактах не проходит (интро B, половина трека) \
                 или сильная доля раньше середины A"
                    .into()
            }
        }
    };
    Diagnosis {
        mode,
        ratio,
        keys_ok,
        reason,
    }
}

struct ClipStats {
    peak_db: f32,
    /// Samples at or above −0.2 dBFS (the soft limiter keeps everything below 1.0).
    hot: usize,
    /// Quietest 50 ms window of the overlap (±¼ s), and the median of the A-only and B-only
    /// parts of the clip (1 s away from the overlap).
    seam_min_db: f32,
    a_db: f32,
    b_db: f32,
    /// Windows below −50 dBFS in the overlap (±¼ s).
    silent: usize,
}

impl ClipStats {
    fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.hot > 0 {
            w.push(format!("{} сэмплов ≥ −0.2 dBFS", self.hot));
        }
        if self.silent > 0 {
            w.push(format!("тишина в переходе: {} мс", self.silent * 50));
        }
        let dip = self.seam_min_db - self.a_db.min(self.b_db);
        if dip < -20.0 {
            w.push(format!("провал в переходе {dip:.1} дБ к тише из A/B"));
        }
        w
    }
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn clip_stats(r: &Rendered) -> ClipStats {
    let frames = r.pcm.len() / 2;
    let win = (r.rate / 20) as usize;
    let rms = |from: usize| -> f32 {
        let to = (from + win).min(frames);
        let s = &r.pcm[from * 2..to * 2];
        (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
    };
    let windows = |from: usize, to: usize| -> Vec<f32> {
        (from..to.min(frames).saturating_sub(win))
            .step_by(win)
            .map(rms)
            .collect()
    };
    let median = |mut v: Vec<f32>| -> f32 {
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let t = r.transition_at as usize;
    let end = t + r.overlap as usize;
    let sec = r.rate as usize;
    // The overlap itself, plus a quarter second on each side for a gap at the joins.
    let seam = windows(t.saturating_sub(sec / 4), end + sec / 4);
    ClipStats {
        peak_db: db(r.pcm.iter().fold(0f32, |m, x| m.max(x.abs()))),
        hot: r.pcm.iter().filter(|x| x.abs() >= 0.977).count(),
        seam_min_db: db(seam.iter().copied().fold(f32::INFINITY, f32::min)),
        a_db: db(median(windows(0, t.saturating_sub(sec)))),
        b_db: db(median(windows(end + sec, frames))),
        silent: seam.iter().filter(|&&x| db(x) < -50.0).count(),
    }
}

fn write_wav(path: &Path, r: &Rendered) {
    let mut w = BufWriter::new(fs::File::create(path).expect("create wav"));
    let data = (r.pcm.len() * 2) as u32;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&r.rate.to_le_bytes());
    h.extend_from_slice(&(r.rate * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data.to_le_bytes());
    w.write_all(&h).expect("wav header");
    for &x in &r.pcm {
        let s = (x.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        w.write_all(&s.to_le_bytes()).expect("wav data");
    }
    w.flush().expect("flush wav");
}

struct Clip {
    file: String,
    rendered: Rendered,
    stats: ClipStats,
    render_ms: u128,
}

struct Card<'a> {
    n: usize,
    a: &'a Track,
    b: &'a Track,
    diag: Diagnosis,
    smart: Clip,
    plain: Clip,
}

fn render_clip(n: usize, a: &Track, b: &Track, smart: bool, opts: &Opts, out: &Path) -> Clip {
    let t0 = Instant::now();
    let r = bench::render_transition(
        a.source(),
        b.source(),
        smart,
        opts.xfade_ms,
        opts.pre_ms,
        opts.post_ms,
    )
    .unwrap_or_else(|e| panic!("render {} → {}: {e}", a.spec.id, b.spec.id));
    let render_ms = t0.elapsed().as_millis();
    let file = format!(
        "{n:02}_{}__{}_{}.wav",
        a.spec.id,
        b.spec.id,
        if smart { "smart" } else { "xfade" }
    );
    write_wav(&out.join("clips").join(&file), &r);
    Clip {
        file,
        stats: clip_stats(&r),
        rendered: r,
        render_ms,
    }
}

fn render_pair<'a>(n: usize, a: &'a Track, b: &'a Track, opts: &Opts, out: &Path) -> Card<'a> {
    let smart = render_clip(n, a, b, true, opts, out);
    let plain = render_clip(n, a, b, false, opts, out);
    let diag = diagnose(a, b, smart.rendered.smart.as_ref());
    // The engine's plan must be what the planner gives for the same inputs.
    let check = bench::plan(&a.analysis, &b.analysis, a.duration_ms, b.duration_ms);
    if check.as_ref().map(|p| (p.start, p.len))
        != smart.rendered.smart.as_ref().map(|p| (p.start, p.len))
    {
        eprintln!(
            "пара {n}: план движка {:?} ≠ плана по probe-длине {:?} (длительности расходятся)",
            smart.rendered.smart, check
        );
    }
    println!(
        "{n:>2}. {} → {}: {} ({} мс + {} мс рендер)",
        a.spec.id, b.spec.id, diag.reason, smart.render_ms, plain.render_ms
    );
    Card {
        n,
        a,
        b,
        diag,
        smart,
        plain,
    }
}

fn print_pair_table(cards: &[Card]) {
    println!(
        "\n{:>2} {:<36} {:<9} {:>9} {:>5} {:>8} {:>5} {:>7} {:>7} {:>7} {:<}",
        "#", "pair", "mode", "startA", "bars", "speed", "keys", "peak", "seam", "A/B", "warnings"
    );
    for c in cards {
        let r = &c.smart.rendered;
        let bar_ms = c.a.analysis.beat_period_ms().map(|p| 4.0 * p);
        let len_ms = r.overlap as f64 * 1000.0 / r.rate as f64;
        let s = &c.smart.stats;
        let mut warn = s.warnings();
        warn.extend(
            c.plain
                .stats
                .warnings()
                .into_iter()
                .map(|w| format!("xfade: {w}")),
        );
        println!(
            "{:>2} {:<36} {:<9} {:>9} {:>5} {:>8} {:>5} {:>7.1} {:>7.1} {:>7} {}",
            c.n,
            format!("{} → {}", c.a.spec.id, c.b.spec.id),
            match c.diag.mode {
                Mode::Synced => "smart",
                Mode::Unsynced => "unsynced",
                Mode::Fallback => "fallback",
            },
            r.smart
                .as_ref()
                .map_or("—".into(), |p| secs(p.start * 1000 / r.rate as u64)),
            match (c.diag.mode, bar_ms) {
                (Mode::Synced, Some(b)) => format!("{:.0}", len_ms / b),
                _ => "—".into(),
            },
            r.smart
                .as_ref()
                .map_or("—".into(), |p| format!("{:.4}", p.speed)),
            match c.diag.keys_ok {
                Some(true) => "ok",
                Some(false) => "clash",
                None => "?",
            },
            s.peak_db,
            s.seam_min_db,
            format!("{:.0}/{:.0}", s.a_db, s.b_db),
            warn.join("; ")
        );
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn report(tracks: &[Track], cards: &[Card], opts: &Opts) -> String {
    let mut h = String::new();
    h.push_str(
        r#"<!doctype html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Mixbench — умные переходы</title>
<style>
:root { color-scheme: dark; --bg:#121214; --card:#1c1c20; --line:#2c2c33; --text:#e8e8ec; --dim:#9a9aa6;
  --smart:#7cc4ff; --plain:#c9a4ff; --ok:#6fd38f; --warn:#ffb454; --bad:#ff6b6b; }
* { box-sizing: border-box; }
body { margin:0; background:var(--bg); color:var(--text); font:14px/1.45 system-ui, "Segoe UI", sans-serif; }
main { max-width: 1100px; margin: 0 auto; padding: 24px 16px 64px; }
h1 { font-size: 22px; margin: 0 0 4px; }
h2 { font-size: 16px; margin: 32px 0 12px; color: var(--dim); font-weight: 600; }
.dim { color: var(--dim); }
.tablewrap { overflow-x: auto; border: 1px solid var(--line); border-radius: 10px; }
table { border-collapse: collapse; width: 100%; font-size: 13px; }
th, td { padding: 6px 10px; text-align: left; border-bottom: 1px solid var(--line); white-space: nowrap; }
th { color: var(--dim); font-weight: 600; background: #18181c; }
td.num { text-align: right; font-variant-numeric: tabular-nums; }
.ok { color: var(--ok); } .warn { color: var(--warn); } .bad { color: var(--bad); }
.card { background: var(--card); border: 1px solid var(--line); border-radius: 12px; padding: 16px; margin: 0 0 16px; }
.card h3 { margin: 0 0 2px; font-size: 16px; }
.badge { display:inline-block; padding: 1px 8px; border-radius: 999px; font-size: 12px; font-weight: 600; margin-left: 6px; vertical-align: 2px; }
.b-smart { background:#183447; color: var(--smart); } .b-uns { background:#3d2f17; color: var(--warn); } .b-fb { background:#3f1d1d; color: var(--bad); }
.tracks { display:grid; grid-template-columns: 1fr 1fr; gap: 8px 16px; margin: 10px 0; }
.tracks div { background:#16161a; border-radius: 8px; padding: 8px 10px; }
.facts { display:flex; flex-wrap: wrap; gap: 4px 18px; margin: 8px 0 12px; color: var(--dim); }
.facts b { color: var(--text); font-weight: 600; }
.player { margin: 10px 0 4px; }
.player .label { font-weight: 600; margin-bottom: 4px; }
.player audio { width: 100%; height: 36px; }
.bar { position: relative; height: 8px; background: #26262c; border-radius: 4px; margin: 6px 0 2px; }
.bar span { position: absolute; top: 0; bottom: 0; border-radius: 4px; }
.row { display:flex; gap: 8px; align-items: center; flex-wrap: wrap; font-size: 12px; color: var(--dim); }
button { background:#26262c; color: var(--text); border: 1px solid var(--line); border-radius: 6px; padding: 2px 8px; font: inherit; font-size: 12px; cursor: pointer; }
button:hover { border-color: #44444f; }
.warnings { color: var(--warn); font-size: 12px; margin-top: 4px; }
@media (max-width: 640px) { .tracks { grid-template-columns: 1fr; } }
</style>
</head>
<body><main>
<h1>Умные переходы (бета) — стенд прослушивания</h1>
"#,
    );
    let _ = writeln!(
        h,
        "<p class=\"dim\">Каждая пара отрендерена движком без звуковой карты: «умный» переход (анализ + план + вариспид + смена басов) \
         и обычный кроссфейд {} с для сравнения. В клипе {} с до начала перехода и {} с после его конца. \
         Нормализация громкости включена (−14 LUFS), 48 кГц, 16 бит.</p>",
        opts.xfade_ms / 1000,
        opts.pre_ms / 1000,
        opts.post_ms / 1000
    );

    h.push_str("<h2>Треки</h2><div class=\"tablewrap\"><table><tr><th>Трек</th><th>Жанр</th><th>Ожид. BPM</th><th>BPM</th><th title=\"Лучшая сетка до порогов устойчивости\">BPM до порога</th><th>Проверка</th>\
        <th title=\"Автокорреляция на периоде доли; порог 0.15\">Периодичность</th><th title=\"Сила онсетов на сетке; порог 1.5\">Сила доли</th>\
        <th>Тональность</th><th>Интро до</th><th>Аутро с</th><th>Длина</th><th>LUFS</th><th>Анализ, мс</th></tr>");
    for t in tracks {
        let a = &t.analysis;
        let v = t.tempo_verdict();
        let cls = if v == "ok" {
            "ok"
        } else if v == "эталона нет" {
            "dim"
        } else if v.starts_with("отброшен") {
            "warn"
        } else {
            "bad"
        };
        let detail = |f: fn(&TempoDetail) -> f64, min: f64| match t.detail {
            Some(d) => {
                let x = f(&d);
                format!(
                    "<td class=\"num {}\">{x:.2}</td>",
                    if x < min { "bad" } else { "" }
                )
            }
            None => "<td class=\"num\">—</td>".into(),
        };
        let _ = write!(
            h,
            "<tr><td><a href=\"{}\" style=\"color:inherit\">{}</a> <span class=\"dim\">— {}</span></td><td>{}</td><td class=\"num\">{}</td>\
             <td class=\"num\">{}</td><td class=\"num dim\">{}</td><td class=\"{cls}\">{}</td>{}{}<td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>\
             <td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td></tr>",
            esc(&t.spec.page),
            esc(&t.spec.title),
            esc(&t.spec.artist),
            esc(&t.spec.genre),
            t.spec.expected_bpm.map_or("?".into(), |b| format!("{b}")),
            if a.bpm > 0.0 {
                format!("{:.1}", a.bpm)
            } else {
                "—".into()
            },
            t.detail.map_or("—".into(), |d| format!("{:.1}", d.bpm)),
            esc(&v),
            detail(|d| d.periodicity, 0.15),
            detail(|d| d.strength, 1.5),
            key_name(a.key),
            secs(a.intro_end_ms),
            secs(a.outro_start_ms),
            secs(t.duration_ms),
            t.lufs.map_or("—".into(), |l| format!("{l:.1}")),
            t.analysis_ms,
        );
    }
    h.push_str("</table></div>");
    h.push_str("<p class=\"dim\">Периодичность и сила доли показаны и тогда, когда анализ их отбросил (красным — ниже порога, тогда BPM = 0 и переход без сведения).</p>");

    h.push_str("<h2>Пары</h2>");
    for c in cards {
        card(&mut h, c, opts);
    }
    h.push_str(
        r#"<script>
document.querySelectorAll('[data-seek]').forEach(b => b.addEventListener('click', () => {
  const a = document.getElementById(b.dataset.audio);
  document.querySelectorAll('audio').forEach(x => { if (x !== a) x.pause(); });
  a.currentTime = parseFloat(b.dataset.seek);
  a.play();
}));
document.querySelectorAll('audio').forEach(a => a.addEventListener('play', () => {
  document.querySelectorAll('audio').forEach(x => { if (x !== a) x.pause(); });
}));
</script>
</main></body></html>
"#,
    );
    h
}

fn card(h: &mut String, c: &Card, opts: &Opts) {
    let (badge, cls) = match c.diag.mode {
        Mode::Synced => ("умный, доли сведены", "b-smart"),
        Mode::Unsynced => ("умный без сведения", "b-uns"),
        Mode::Fallback => ("откат на кроссфейд", "b-fb"),
    };
    let _ = write!(
        h,
        "<section class=\"card\"><h3>{}. {} → {}<span class=\"badge {cls}\">{badge}</span></h3>",
        c.n,
        esc(&c.a.spec.genre),
        esc(&c.b.spec.genre)
    );
    h.push_str("<div class=\"tracks\">");
    for (label, t) in [("A", c.a), ("B", c.b)] {
        let a = &t.analysis;
        let _ = write!(
            h,
            "<div><b>{label}:</b> {} <span class=\"dim\">— {}</span><br><span class=\"dim\">{} · BPM <b>{}</b> · доля {} · {} · интро до {} · аутро с {} · {} · {}</span></div>",
            esc(&t.spec.title),
            esc(&t.spec.artist),
            esc(&t.spec.genre),
            if a.bpm > 0.0 {
                format!("{:.1}", a.bpm)
            } else {
                "—".into()
            },
            t.detail.map_or("—".into(), |d| format!(
                "период. {:.2} / сила {:.2}",
                d.periodicity, d.strength
            )),
            key_name(a.key),
            secs(a.intro_end_ms),
            secs(a.outro_start_ms),
            secs(t.duration_ms),
            esc(&t.spec.licence),
        );
    }
    h.push_str("</div>");

    let r = &c.smart.rendered;
    let rate = r.rate as f64;
    let mut facts = Vec::new();
    match &r.smart {
        Some(p) => {
            facts.push(format!(
                "старт в A <b>{}</b>",
                secs((p.start as f64 * 1000.0 / rate) as u64)
            ));
            let len_ms = p.len as f64 * 1000.0 / rate;
            match (c.diag.mode, c.a.analysis.beat_period_ms()) {
                (Mode::Synced, Some(pa)) => facts.push(format!(
                    "длина <b>{:.0} тактов</b> ({:.1} с)",
                    len_ms / (4.0 * pa),
                    len_ms / 1000.0
                )),
                _ => facts.push(format!("длина <b>{:.1} с</b>", len_ms / 1000.0)),
            }
            facts.push(format!("B с <b>{}</b>", secs(p.b_start_ms)));
            facts.push(format!(
                "скорость B <b>×{:.4}</b> ({:+.2} %, возврат за {:.1} с)",
                p.speed,
                (p.speed - 1.0) * 100.0,
                p.ramp as f64 / rate
            ));
            facts.push(format!(
                "смена басов {:.1}–{:.1} с перехода",
                p.swap_at as f64 / rate,
                (p.swap_at + p.swap_len) as f64 / rate
            ));
        }
        None => facts.push(format!(
            "кроссфейд {} с в самом конце A",
            opts.xfade_ms / 1000
        )),
    }
    if let Some((s, m)) = c.diag.ratio {
        facts.push(format!(
            "темп B/A{} = <b>{:.3}</b>",
            match m {
                0.5 => " (½)",
                2.0 => " (×2)",
                _ => "",
            },
            1.0 / s
        ));
    }
    facts.push(format!(
        "тональности: <b>{}</b>",
        match c.diag.keys_ok {
            Some(true) => "сочетаются",
            Some(false) => "конфликт",
            None => "неизвестно",
        }
    ));
    let _ = write!(
        h,
        "<div class=\"facts\"><span>{}</span></div><div class=\"dim\" style=\"font-size:12px\">{}</div>",
        facts.join("</span><span>"),
        esc(&c.diag.reason)
    );

    for (clip, label, color, id) in [
        (
            &c.smart,
            "Умный переход",
            "var(--smart)",
            format!("s{}", c.n),
        ),
        (
            &c.plain,
            "Обычный кроссфейд",
            "var(--plain)",
            format!("p{}", c.n),
        ),
    ] {
        let r = &clip.rendered;
        let total = r.pcm.len() as f64 / 2.0 / rate;
        let at = r.transition_at as f64 / rate;
        let len = r.overlap as f64 / rate;
        let s = &clip.stats;
        let _ = write!(
            h,
            "<div class=\"player\"><div class=\"label\" style=\"color:{color}\">{label}</div>\
             <audio id=\"{id}\" controls preload=\"none\" src=\"clips/{}\"></audio>\
             <div class=\"bar\"><span style=\"left:{:.2}%;width:{:.2}%;background:{color};opacity:.55\"></span></div>\
             <div class=\"row\"><span>переход с <b style=\"color:var(--text)\">{at:.1} с</b> до {:.1} с клипа (A с {} трека)</span>\
             <button data-audio=\"{id}\" data-seek=\"{:.2}\">−5 с до перехода</button>\
             <button data-audio=\"{id}\" data-seek=\"{:.2}\">середина</button>\
             <span>пик {:.1} dBFS · минимум у шва {:.1} dB · A {:.1} / B {:.1} dB RMS</span></div>",
            esc(&clip.file),
            at / total * 100.0,
            (len / total * 100.0).max(0.4),
            at + len,
            secs(r.a_from_ms),
            (at - 5.0).max(0.0),
            at + len / 2.0,
            s.peak_db,
            s.seam_min_db,
            s.a_db,
            s.b_db,
        );
        let w = s.warnings();
        if !w.is_empty() {
            let _ = write!(h, "<div class=\"warnings\">⚠ {}</div>", esc(&w.join("; ")));
        }
        h.push_str("</div>");
    }
    h.push_str("</section>");
}
