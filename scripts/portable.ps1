# Builds the portable Windows release: a zip with gall.exe and a `portable` marker,
# so settings, data, logs and the webview's data stay in .\UserData next to the exe.
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
  New-Item -ItemType File (Join-Path $stage "portable") | Out-Null
  Copy-Item LICENSE, THIRD-PARTY-NOTICES.md $stage
  @"
Gall $version, portable

Run gall.exe. Nothing is installed and nothing is written to your user profile:
settings, data, logs and the browser engine's cache live in the UserData folder
next to gall.exe, so the whole folder can be moved or kept on a USB drive.

Delete the file named "portable" to use the normal per-user locations instead
(%APPDATA%\gall and %LOCALAPPDATA%).

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
