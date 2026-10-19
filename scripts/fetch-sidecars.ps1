# Downloads the external binaries Mixtape runs as sidecars (yt-dlp and deno)
# into src-tauri/binaries with the target-triple suffix Tauri expects.
#
# Versions are pinned and every download is checked against the SHA-256 below (taken from
# the projects' own release checksums: yt-dlp's SHA2-256SUMS, deno's *.zip.sha256sum).
# To update: change the version, download the new checksum from the release page and
# paste it here in the same commit.
$ErrorActionPreference = 'Stop'

$YtDlpVersion = '2026.08.19'
$YtDlpSha256 = '66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a'  # yt-dlp.exe
$DenoVersion = '2.9.7'
$DenoZipSha256 = 'a0c3101b4158d1dfb7d6a78a7bf0f3de80c96bb423c152beec8beb22786f2238'  # deno-x86_64-pc-windows-msvc.zip

$triple = 'x86_64-pc-windows-msvc'
$dir = Join-Path $PSScriptRoot '..\src-tauri\binaries'
New-Item -ItemType Directory -Force $dir | Out-Null

function Assert-Sha256([string]$Path, [string]$Expected) {
    $actual = (Get-FileHash -Algorithm SHA256 $Path).Hash.ToLowerInvariant()
    if ($actual -ne $Expected.ToLowerInvariant()) {
        Remove-Item $Path -Force -ErrorAction SilentlyContinue
        throw "SHA-256 mismatch for $(Split-Path $Path -Leaf): expected $Expected, got $actual"
    }
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("mixtape-sidecars-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $work | Out-Null
try {
    Write-Host "Downloading yt-dlp $YtDlpVersion..."
    $ytdlpTmp = Join-Path $work 'yt-dlp.exe'
    Invoke-WebRequest "https://github.com/yt-dlp/yt-dlp/releases/download/$YtDlpVersion/yt-dlp.exe" -OutFile $ytdlpTmp
    Assert-Sha256 $ytdlpTmp $YtDlpSha256

    Write-Host "Downloading deno $DenoVersion..."
    $zip = Join-Path $work 'deno.zip'
    Invoke-WebRequest "https://github.com/denoland/deno/releases/download/v$DenoVersion/deno-$triple.zip" -OutFile $zip
    Assert-Sha256 $zip $DenoZipSha256
    $unzipped = Join-Path $work 'deno'
    Expand-Archive $zip -DestinationPath $unzipped -Force

    $ytdlp = Join-Path $dir "yt-dlp-$triple.exe"
    $deno = Join-Path $dir "deno-$triple.exe"
    Move-Item $ytdlpTmp $ytdlp -Force
    Move-Item (Join-Path $unzipped 'deno.exe') $deno -Force
}
finally {
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
}

& $ytdlp --version
& $deno --version | Select-Object -First 1
