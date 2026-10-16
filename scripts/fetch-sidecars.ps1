# Downloads the external binaries Mixtape runs as sidecars (yt-dlp and deno)
# into src-tauri/binaries with the target-triple suffix Tauri expects.
$ErrorActionPreference = 'Stop'
$triple = 'x86_64-pc-windows-msvc'
$dir = Join-Path $PSScriptRoot '..\src-tauri\binaries'
New-Item -ItemType Directory -Force $dir | Out-Null

$ytdlp = Join-Path $dir "yt-dlp-$triple.exe"
Write-Host 'Downloading yt-dlp...'
Invoke-WebRequest 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe' -OutFile $ytdlp

$deno = Join-Path $dir "deno-$triple.exe"
$zip = Join-Path ([IO.Path]::GetTempPath()) 'mixtape-deno.zip'
Write-Host 'Downloading deno...'
Invoke-WebRequest "https://github.com/denoland/deno/releases/latest/download/deno-$triple.zip" -OutFile $zip
$tmp = Join-Path ([IO.Path]::GetTempPath()) 'mixtape-deno'
Expand-Archive $zip -DestinationPath $tmp -Force
Move-Item (Join-Path $tmp 'deno.exe') $deno -Force
Remove-Item $zip, $tmp -Recurse -Force

& $ytdlp --version
& $deno --version | Select-Object -First 1
