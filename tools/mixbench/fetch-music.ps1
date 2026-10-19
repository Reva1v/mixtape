# Downloads the bench tracks listed in tracks.json into music/ (gitignored) and writes
# music/SOURCES.md. Already downloaded files are skipped; -Force downloads them again.
#   pwsh tools/mixbench/fetch-music.ps1
param([switch]$Force)
$ErrorActionPreference = 'Stop'

$here = $PSScriptRoot
$root = Resolve-Path (Join-Path $here '..\..')
$ytdlp = Join-Path $root 'src-tauri\binaries\yt-dlp-x86_64-pc-windows-msvc.exe'
$music = Join-Path $here 'music'
New-Item -ItemType Directory -Force $music | Out-Null
$spec = Get-Content (Join-Path $here 'tracks.json') -Raw | ConvertFrom-Json

foreach ($t in $spec.tracks) {
    $dest = Join-Path $music $t.file
    if ((Test-Path $dest) -and -not $Force) {
        Write-Host "ok    $($t.file)"
        continue
    }
    Write-Host "fetch $($t.file)"
    $tmp = "$dest.part"
    Remove-Item $tmp -ErrorAction SilentlyContinue
    if ($t.download.url) {
        # ccMixter answers 403 without the track page as the referer.
        Invoke-WebRequest -Uri $t.download.url -OutFile $tmp -UserAgent 'Mozilla/5.0' -Headers @{ Referer = $t.page } -MaximumRetryCount 4 -RetryIntervalSec 3
    } else {
        if (-not (Test-Path $ytdlp)) { throw "yt-dlp not found at $ytdlp (scripts/fetch-sidecars.ps1)" }
        # SoundCloud serves AAC over HLS: the fragments join into a fragmented MP4 without ffmpeg.
        & $ytdlp --no-part --no-playlist -f 'hls_aac_160k/hls_aac_96k/http_mp3_128/bestaudio' -o $tmp $t.download.ytdlp
        if ($LASTEXITCODE -ne 0) { throw "yt-dlp failed for $($t.id)" }
    }
    Move-Item -Force $tmp $dest
}

$lines = @('# Bench music sources', '', 'Downloaded by `fetch-music.ps1` from `tracks.json`. Personal listening tests only.', '',
    '| File | Title | Artist | Genre | Expected BPM | Licence | Source |', '|---|---|---|---|---|---|---|')
foreach ($t in $spec.tracks) {
    $bpm = if ($null -ne $t.expected_bpm) { $t.expected_bpm } else { '?' }
    $lines += "| $($t.file) | $($t.title) | $($t.artist) | $($t.genre) | $bpm | $($t.licence) | $($t.page) |"
}
Set-Content -Path (Join-Path $music 'SOURCES.md') -Value $lines -Encoding utf8
Write-Host "done: $($spec.tracks.Count) tracks in $music"
