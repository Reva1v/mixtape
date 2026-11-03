# Logs the session's state every 2 minutes, one line per check: read-only, values never printed.
param([string]$Log, [int]$Hours = 6)
Add-Type -AssemblyName System.Security
$file = "$env:LOCALAPPDATA\Mixtape\account.bin"
$origin = 'https://music.youtube.com'
$ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36'
$ver = '1.20261013.01.00'
$end = (Get-Date).AddHours($Hours)
$was = $null
while ((Get-Date) -lt $end) {
    try {
        $text = [Text.Encoding]::UTF8.GetString([Security.Cryptography.ProtectedData]::Unprotect([IO.File]::ReadAllBytes($file), $null, 'CurrentUser'))
        $jar = [ordered]@{}; $sidExp = 0
        foreach ($l in $text -split "`n") {
            if ($l -match '^#' -or -not $l.Trim()) { continue }
            $f = $l.TrimEnd("`r") -split "`t"
            if ($f.Count -ge 7 -and -not $jar.Contains($f[5])) { $jar[$f[5]] = $f[6]; if ($f[5] -eq 'SID') { $sidExp = $f[4] } }
        }
        $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
        $hash = { param($v) "${now}_" + (-join ([Security.Cryptography.SHA1]::HashData([Text.Encoding]::UTF8.GetBytes("$now $v $origin")) | ForEach-Object { $_.ToString('x2') })) }
        $auth = "SAPISIDHASH $(& $hash $jar['SAPISID']) SAPISID1PHASH $(& $hash $jar['__Secure-1PAPISID']) SAPISID3PHASH $(& $hash $jar['__Secure-3PAPISID'])"
        $body = @{ browseId = 'VLLM'; context = @{ client = @{ clientName = 'WEB_REMIX'; clientVersion = $ver; hl = 'ru'; gl = 'RU' }; user = @{} } } | ConvertTo-Json -Depth 6
        $h = @{ Cookie = (($jar.Keys | ForEach-Object { "$_=$($jar[$_])" }) -join '; '); Authorization = $auth; Origin = $origin; 'X-Origin' = $origin; Referer = "$origin/"; 'X-Goog-AuthUser' = '0'; 'X-Youtube-Client-Name' = '67'; 'X-Youtube-Client-Version' = $ver }
        $r = Invoke-WebRequest -Uri "$origin/youtubei/v1/browse?prettyPrint=false" -Method Post -Headers $h -UserAgent $ua -ContentType 'application/json' -Body $body -SkipHttpErrorCheck -TimeoutSec 30
        $m = [regex]::Match($r.Content, '"key":"logged_in","value":"(\d)"').Groups[1].Value
        $rts = $jar.Contains('__Secure-1PSIDRTS')
        $procs = (Get-Process mixtape-player, droplet-island -ErrorAction SilentlyContinue | ForEach-Object ProcessName | Sort-Object -Unique) -join '+'
        $line = "{0:HH:mm:ss} logged_in={1} rts={2} session={3} file={4:HH:mm:ss} running={5}" -f (Get-Date), $m, $rts, $sidExp, (Get-Item $file).LastWriteTime, $procs
    } catch { $line = "{0:HH:mm:ss} error {1}" -f (Get-Date), $_.Exception.GetType().Name }
    Add-Content $Log $line
    Start-Sleep -Seconds 120
}
