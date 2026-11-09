param(
    [string]$Binary = (Join-Path $PSScriptRoot "..\target\debug\gall.exe")
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path $Binary).Path
$root = Join-Path ([System.IO.Path]::GetTempPath()) ("gall-quick-open-selftest-" + [guid]::NewGuid().ToString("N"))
$app = Join-Path $root "gall.exe"
$data = Join-Path $root "TestData"
$out = Join-Path $root "out"
New-Item -ItemType Directory -Path $data, $out, (Join-Path $data "Projects"), (Join-Path $root "UserData") | Out-Null
Copy-Item -LiteralPath $source -Destination $app
Set-Content -LiteralPath (Join-Path $root "portable") -Value "" -NoNewline
$names = @("GALL_MULTI_INSTANCE", "GALL_SELFTEST", "GALL_SELFTEST_DIR")
$previous = @{}
foreach ($name in $names) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, "Process") }
try {
    $env:GALL_MULTI_INSTANCE = "1"
    $env:GALL_SELFTEST = "quick-open"
    $env:GALL_SELFTEST_DIR = $out
    $process = Start-Process -FilePath $app -ArgumentList @("-p", "`"$data`"", "-p2", "`"$(Join-Path $data 'Projects')`"") -PassThru -RedirectStandardOutput (Join-Path $root "stdout.log") -RedirectStandardError (Join-Path $root "stderr.log")
    if (!$process.WaitForExit(120000)) {
        Stop-Process -Id $process.Id
        throw "Quick Open self-test timed out. Artifacts: $root"
    }
    $report = Join-Path $out "report.txt"
    if (!(Test-Path -LiteralPath $report)) { throw "No self-test report. Inspect $root\stderr.log" }
    Get-Content -LiteralPath $report
    Write-Output "Artifacts: $root"
    if (!(Select-String -LiteralPath $report -Pattern "^0 failure" -Quiet)) { throw "Quick Open self-test failed. Artifacts: $root" }
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $previous[$name], "Process") }
}
