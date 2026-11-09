param([switch]$PackageOnly)
$ErrorActionPreference = 'Stop'
$mediaRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
Push-Location -LiteralPath $mediaRoot
try {
    $env:CARGO_ENCODED_RUSTFLAGS = @(
        '-C', 'target-feature=+crt-static',
        "--remap-path-prefix=$env:USERPROFILE=/user",
        "--remap-path-prefix=$mediaRoot=/taskbar-media"
    ) -join [char]31
    if (-not $PackageOnly) {
        $ErrorActionPreference = 'Continue'
        & cargo build --release --locked
        $buildExit = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        if ($buildExit -ne 0) {
            throw 'Release build failed. Close standalone views and disable the band. Explorer can retain its DLL until sign-out or an explicitly requested shell restart.'
        }
    }
    $outputs = @('taskbar-media.exe', 'taskbar-media-ui.exe', 'taskbar_media_band.dll')
    foreach ($name in $outputs) {
        $bytes = [IO.File]::ReadAllBytes((Join-Path $mediaRoot "target\release\$name"))
        foreach ($encoding in @([Text.Encoding]::UTF8, [Text.Encoding]::Unicode)) {
            if ($encoding.GetString($bytes).Contains($env:USERPROFILE)) { throw "Local profile path remains in $name" }
        }
    }
    $versionMatch = [regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version\s*=\s*"([0-9.]+)"')
    if (-not $versionMatch.Success) { throw 'Cannot read release version' }
    $archive = "taskbar-media-$($versionMatch.Groups[1].Value)-windows-x64.zip"
    $destination = Join-Path $mediaRoot 'dist'
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    $files = @($outputs | ForEach-Object { Join-Path $mediaRoot "target\release\$_" })
    $files += @('README.md','README.ru.md','CHANGELOG.md','LICENSE-MIT','LICENSE-APACHE') | ForEach-Object { Join-Path $mediaRoot $_ }
    $archivePath = Join-Path $destination $archive
    Compress-Archive -LiteralPath $files -DestinationPath $archivePath -Force
    $checksum = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$archivePath.sha256", "$checksum  $archive`n", [Text.UTF8Encoding]::new($false))
    Write-Host $archivePath
    Write-Host "SHA256 $checksum"
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags
    Pop-Location
}
