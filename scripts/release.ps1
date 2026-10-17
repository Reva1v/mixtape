# Builds the signed installer and publishes a GitHub release with `latest.json` for the
# in-app updater. Bump "version" in src-tauri/tauri.conf.json (and Cargo.toml) first.
#
#   ./scripts/release.ps1 -Notes release-notes.md
#
# The updater signing key lives in .secrets/tauri-updater.key (gitignored). Keep a backup:
# without it, installed copies can't verify new versions.
param(
    [Parameter(Mandatory)][string]$Notes,
    [string]$Repo = 'Reva1v/mixtape'
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$conf = Get-Content (Join-Path $root 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$version = $conf.version
$tag = "v$version"
$exeName = "$($conf.productName)_$($version)_x64-setup.exe"

$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content (Join-Path $root '.secrets\tauri-updater.key') -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ''
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '4' }

Push-Location $root
try {
    # Signed updater artifacts only here, so a plain `pnpm tauri build` works without the key.
    pnpm tauri build --config src-tauri/tauri.release.conf.json
    if ($LASTEXITCODE) { throw "build failed" }

    $bundle = Join-Path $root 'target\release\bundle\nsis'
    $exe = Join-Path $bundle $exeName
    $sig = Get-Content "$exe.sig" -Raw
    $manifest = [ordered]@{
        version   = $version
        notes     = (Get-Content $Notes -Raw)
        pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        platforms = [ordered]@{
            'windows-x86_64' = [ordered]@{
                signature = $sig.Trim()
                url       = "https://github.com/$Repo/releases/download/$tag/$exeName"
            }
        }
    }
    $latest = Join-Path $bundle 'latest.json'
    $manifest | ConvertTo-Json -Depth 5 | Set-Content $latest -Encoding utf8NoBOM

    git tag -a $tag -m "Mixtape $version"
    git push origin $tag
    gh release create $tag $exe $latest --repo $Repo --title "Mixtape $version" --notes-file $Notes
    if ($LASTEXITCODE) { throw "gh release failed" }
    Write-Host "Released $tag"
}
finally {
    Pop-Location
}
