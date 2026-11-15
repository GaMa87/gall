# Builds the portable Windows release: a zip with gall.exe. Settings stay in the user
# profile so a new version finds them; an empty `portable` file next to the exe switches to .\UserData.
#   ./scripts/portable.ps1            build, then pack
#   ./scripts/portable.ps1 -NoBuild   pack an existing target\release\gall.exe
# Output: dist\Gall-<version>-windows-x64-portable.zip
param([switch]$NoBuild)
$ErrorActionPreference = "Stop"
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
try {
  $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
  if (-not $NoBuild) {
    # Release exe only: the frontend is embedded in it, no installer needed.
    & .\node_modules\.bin\tauri.cmd build --no-bundle
    if ($LASTEXITCODE) { throw "tauri build failed ($LASTEXITCODE)" }
  }
  $exe = Join-Path $repo "target\release\gall.exe"
  if (-not (Test-Path $exe)) { throw "$exe not found; run without -NoBuild" }

  $name = "Gall-$version-windows-x64-portable"
  $stage = Join-Path $repo "dist\$name"
  if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
  New-Item -ItemType Directory -Force $stage | Out-Null
  Copy-Item $exe (Join-Path $stage "gall.exe")
  Copy-Item LICENSE, THIRD-PARTY-NOTICES.md $stage
  @"
Gall $version, portable

Run gall.exe. Nothing is installed. Settings and data are kept in your user
profile (%APPDATA%\gall and %LOCALAPPDATA%\gall), so a newer version you unzip
elsewhere finds them and you do not have to set everything up again.

To keep everything next to gall.exe instead (for example on a USB drive), create
an empty file named "portable" beside it; settings, data, logs and the browser
engine's cache then live in the UserData folder.

Requires the Microsoft Edge WebView2 Runtime, which Windows 10 and 11 include.
If gall.exe does not start, install it from
https://developer.microsoft.com/microsoft-edge/webview2/

https://github.com/GaMa87/gall
"@ -replace "`r?`n", "`r`n" | Set-Content -Encoding utf8 (Join-Path $stage "README.txt")

  $zip = Join-Path $repo "dist\$name.zip"
  if (Test-Path $zip) { Remove-Item $zip -Force }
  Compress-Archive -Path $stage -DestinationPath $zip
  $mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
  "Built $zip ($mb MB)"
} finally {
  Pop-Location
}
