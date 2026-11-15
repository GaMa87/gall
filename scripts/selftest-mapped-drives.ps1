param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern("^[A-Za-z]:$")]
    [string]$Drive,
    [string]$Binary = (Join-Path $PSScriptRoot "..\target\debug\gall.exe")
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path $Binary).Path
$root = Join-Path ([System.IO.Path]::GetTempPath()) ("gall-mapped-drives-" + [guid]::NewGuid().ToString("N"))
$app = Join-Path $root "gall.exe"
$data = Join-Path (Join-Path $root "TestData") ($Drive.Substring(0, 1).ToUpperInvariant())
$out = Join-Path $root "out"
$mappedRoot = $Drive.ToUpperInvariant() + "\"
$info = [System.IO.DriveInfo]::new($mappedRoot)
if ($info.DriveType -ne [System.IO.DriveType]::Network) { throw "$Drive is not a network drive in this logon session" }
New-Item -ItemType Directory -Path $data, $out, (Join-Path $root "UserData") | Out-Null
Copy-Item -LiteralPath $source -Destination $app
Set-Content -LiteralPath (Join-Path $root "portable") -Value "" -NoNewline
$names = @("GALL_MULTI_INSTANCE", "GALL_SELFTEST", "GALL_SELFTEST_DIR")
$previous = @{}
foreach ($name in $names) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, "Process") }
try {
    $env:GALL_MULTI_INSTANCE = "1"
    $env:GALL_SELFTEST = "mapped-drives"
    $env:GALL_SELFTEST_DIR = $out
    $process = Start-Process -FilePath $app -ArgumentList @("-p", "`"$data`"") -PassThru -RedirectStandardOutput (Join-Path $root "stdout.log") -RedirectStandardError (Join-Path $root "stderr.log")
    if (!$process.WaitForExit(120000)) {
        Stop-Process -Id $process.Id
        throw "Mapped drive self-test timed out. Artifacts: $root"
    }
    $report = Join-Path $out "report.txt"
    if (!(Test-Path -LiteralPath $report)) { throw "No self-test report. Inspect $root\stderr.log" }
    Get-Content -LiteralPath $report
    Write-Output "Artifacts: $root"
    if (!(Select-String -LiteralPath $report -Pattern "^0 failure" -Quiet)) { throw "Mapped drive self-test failed. Artifacts: $root" }
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $previous[$name], "Process") }
}
