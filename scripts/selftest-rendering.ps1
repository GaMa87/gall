<#
Runs the rendering self-test (sharpness of the pane frame, rings, icons and emblems at 100, 125, 150 and 200 %)
on a bundled debug build, in a throw-away portable profile. The scales are derived from the display's own
scale, so a 150 % laptop and a 200 % monitor both get all four. Open the PNGs it prints at the end.

  npm --prefix ui run build; cargo build -p gall --features tauri/custom-protocol
  scripts\selftest-rendering.ps1 [-Binary target\debug\gall.exe] [-Folder <folder to show>] [-UserSettings]

-UserSettings copies your own settings.json and favorites.json (theme, accent, fonts) into the profile, so
the capture looks like your Gall; your session and data are never touched.
#>
param(
    [string]$Binary = (Join-Path $PSScriptRoot "..\target\debug\gall.exe"),
    [string]$Folder = (Join-Path $PSScriptRoot ".."),
    [switch]$UserSettings,
    [int]$TimeoutSeconds = 400
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path $Binary).Path
$folder = (Resolve-Path $Folder).Path
$root = Join-Path ([System.IO.Path]::GetTempPath()) ("gall-rendering-selftest-" + [guid]::NewGuid().ToString("N"))
$app = Join-Path $root "gall.exe"
$data = Join-Path $root "UserData"
$out = Join-Path $root "out"
New-Item -ItemType Directory -Path $data, $out | Out-Null
Copy-Item -LiteralPath $source -Destination $app
Set-Content -LiteralPath (Join-Path $root "portable") -Value "" -NoNewline
if ($UserSettings) {
    foreach ($name in "settings.json", "favorites.json") {
        $file = Join-Path $env:APPDATA "gall\$name"
        if (Test-Path -LiteralPath $file) { Copy-Item -LiteralPath $file -Destination (Join-Path $data $name) }
    }
}
$names = @("GALL_MULTI_INSTANCE", "GALL_SELFTEST", "GALL_SELFTEST_DIR")
$previous = @{}
foreach ($name in $names) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, "Process") }
try {
    $env:GALL_MULTI_INSTANCE = "1"
    $env:GALL_SELFTEST = "rendering"
    $env:GALL_SELFTEST_DIR = $out
    $process = Start-Process -FilePath $app -ArgumentList @("-p", "`"$folder`"", "-p2", "`"$folder`"") -PassThru -RedirectStandardOutput (Join-Path $root "stdout.log") -RedirectStandardError (Join-Path $root "stderr.log")
    if (!$process.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $process.Id -Force
        throw "The rendering self-test timed out. Artifacts: $root"
    }
    $report = Join-Path $out "report.txt"
    if (!(Test-Path -LiteralPath $report)) { throw "No self-test report. Inspect $root\stderr.log" }
    Get-Content -LiteralPath $report
    Write-Output "Screenshots (physical pixels, one per scale and theme): $out"
    if (!(Select-String -LiteralPath $report -Pattern "^0 failure" -Quiet)) { throw "The rendering self-test failed. Artifacts: $root" }
} finally {
    # Newer PowerShell sets an empty variable for a null value.
    foreach ($name in $names) { if ($null -eq $previous[$name]) { Remove-Item -Path "Env:$name" -ErrorAction Ignore } else { Set-Item -Path "Env:$name" -Value $previous[$name] } }
}
