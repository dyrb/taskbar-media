param([string]$BinaryDirectory = $PSScriptRoot)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath (Join-Path $BinaryDirectory 'taskbar-media.exe'))) {
    $BinaryDirectory = Join-Path $PSScriptRoot '..\target\release'
}
$BinaryDirectory = (Resolve-Path -LiteralPath $BinaryDirectory).Path
$mediaCli = Join-Path $BinaryDirectory 'taskbar-media.exe'
$mediaUi = Join-Path $BinaryDirectory 'taskbar-media-ui.exe'
foreach ($mediaFile in @($mediaCli, $mediaUi, (Join-Path $BinaryDirectory 'taskbar_media_band.dll'))) {
    if (-not (Test-Path -LiteralPath $mediaFile -PathType Leaf)) { throw "Missing program file: $mediaFile" }
}
$mediaSession = (Get-Process -Id $PID).SessionId
Get-Process -Name taskbar-media-ui -ErrorAction SilentlyContinue |
    Where-Object { $_.SessionId -eq $mediaSession -and $_.Path -eq $mediaUi } |
    Stop-Process -Force
& $mediaCli disable
if ($LASTEXITCODE -ne 0) { Write-Warning 'The band could not be hidden before restarting Explorer.' }
Get-Process -Name explorer -ErrorAction SilentlyContinue |
    Where-Object { $_.SessionId -eq $mediaSession } |
    Stop-Process -Force
for ($mediaAttempt = 0; $mediaAttempt -lt 20; $mediaAttempt++) {
    Start-Sleep -Milliseconds 100
    $mediaShell = Get-Process -Name explorer -ErrorAction SilentlyContinue |
        Where-Object { $_.SessionId -eq $mediaSession }
    if ($mediaShell) { break }
}
if (-not $mediaShell) { Start-Process -FilePath explorer.exe -WindowStyle Hidden }
$mediaShown = $false
for ($mediaAttempt = 0; $mediaAttempt -lt 15; $mediaAttempt++) {
    Start-Sleep -Milliseconds 500
    & $mediaCli enable
    if ($LASTEXITCODE -eq 0) { $mediaShown = $true; break }
}
if (-not $mediaShown) { throw 'Explorer restarted, but the band could not be enabled. Run taskbar-media-ui.exe or select Taskbar Media in the taskbar Toolbars menu.' }
