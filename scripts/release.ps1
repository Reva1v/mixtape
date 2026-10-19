# Builds the installer, signs it for the in-app updater and publishes a GitHub release with
# `latest.json`. Bump "version" in src-tauri/tauri.conf.json (and Cargo.toml) first.
#
#   ./scripts/release.ps1 -Notes release-notes.md
#
# The updater signing key lives in .secrets/tauri-updater.key (gitignored; override with
# -KeyPath). Keep a backup: without it, installed copies can't verify new versions.
#
# The key never enters the build's environment: the installer is built unsigned (a plain
# `pnpm tauri build`), then signed on its own with `tauri signer sign`. The key password is
# read from TAURI_SIGNING_PRIVATE_KEY_PASSWORD or asked for (press Enter for a key without
# one) and cleared from the environment afterwards.
#
# A new password-protected key (only for a fresh setup: changing the key breaks updates for
# installed copies, since they trust the old public key in tauri.conf.json):
#
#   pnpm tauri signer generate -w .secrets/tauri-updater.key
#   # enter a password when asked, then put the .pub contents into plugins.updater.pubkey
param(
    [Parameter(Mandatory)][string]$Notes,
    [string]$Repo = 'Reva1v/mixtape',
    [string]$KeyPath
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
if (-not $KeyPath) { $KeyPath = Join-Path $root '.secrets\tauri-updater.key' }
if (-not (Test-Path $KeyPath)) { throw "Signing key not found: $KeyPath" }
$conf = Get-Content (Join-Path $root 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$version = $conf.version
$tag = "v$version"
$exeName = "$($conf.productName)_$($version)_x64-setup.exe"
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '4' }

# Never let a key from the caller's shell leak into the build.
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PATH -ErrorAction SilentlyContinue

$password = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
if ($null -eq $password) {
    $secure = Read-Host 'Updater key password (Enter if none)' -AsSecureString
    $password = [Net.NetworkCredential]::new('', $secure).Password
}

Push-Location $root
try {
    pnpm tauri build
    if ($LASTEXITCODE) { throw "build failed" }

    $bundle = Join-Path $root 'target\release\bundle\nsis'
    $exe = Join-Path $bundle $exeName
    if (-not (Test-Path $exe)) { throw "installer not found: $exe" }

    # The password goes to the signer through its own environment only for this call.
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $password
    try {
        pnpm tauri signer sign -f $KeyPath --app-version $version $exe
        if ($LASTEXITCODE) { throw "signing failed" }
    }
    finally {
        Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
    }
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
    $password = $null
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
    Pop-Location
}
