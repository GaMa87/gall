<#
Windows counterpart of `scripts/selftest.sh --popup-placement`: where the menus, submenus and dropdowns opened by a key
appear (docs/popup-placement-audit.md), with Vim keys on and off, at 100, 150 and 200 % (derived from the display's own scale).
Each run uses a throw-away portable profile and disposable data.

  npm --prefix ui run build; cargo build -p gall --features tauri/custom-protocol
  scripts\selftest-popups.ps1 [-Binary target\debug\gall.exe] [-Scales 1,1.5,2] [-OutputFolder <folder>]

The output folder is printed at the end; open every PNG.
#>
param(
    [string]$Binary = (Join-Path $PSScriptRoot "..\target\debug\gall.exe"),
    [double[]]$Scales = @(1, 1.5, 2),
    [string]$OutputFolder = "",
    [int]$TimeoutSeconds = 400
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path $Binary).Path
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("gall-popups-selftest-" + [guid]::NewGuid().ToString("N"))
$results = if ($OutputFolder) { $OutputFolder } else { Join-Path $work "results" }
New-Item -ItemType Directory -Force -Path $work, $results | Out-Null
$results = (Resolve-Path $results).Path
$emptyConfig = Join-Path $work "empty.gitconfig"
Set-Content -LiteralPath $emptyConfig -Value "" -NoNewline

function Invoke-Git([string]$dir, [string[]]$arguments) {
    & git -C $dir -c user.name=Test -c user.email=test@example.com -c commit.gpgSign=false -c core.hooksPath=NUL -c core.autocrlf=false -c protocol.file.allow=always @arguments
    if ($LASTEXITCODE -ne 0) { throw "git $($arguments -join ' ') failed in $dir" }
}

$names = @("GALL_MULTI_INSTANCE", "GALL_SELFTEST", "GALL_SELFTEST_DIR", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM")
$previous = @{}
foreach ($name in $names) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, "Process") }
$failed = $false
try {
    $env:GIT_CONFIG_GLOBAL = $emptyConfig; $env:GIT_CONFIG_NOSYSTEM = "1"
    foreach ($scale in $Scales) {
        $label = $scale.ToString([cultureinfo]::InvariantCulture)
        $root = Join-Path $work ("run-" + ($label -replace '\.', '_'))
        $data = Join-Path $root "TestData"; $repo = Join-Path $data "Projects"; $module = Join-Path $root "module-source"
        $out = Join-Path $results ($label -replace '\.', '_')
        New-Item -ItemType Directory -Force -Path $repo, $module, $out, (Join-Path $data "Documents"), (Join-Path $root "UserData") | Out-Null
        foreach ($i in 1..60) { Set-Content -LiteralPath (Join-Path $data ("Documents\file{0:00}.txt" -f $i)) -Value "line $i" }
        Invoke-Git $module @("init", "-q", "-b", "main")
        Invoke-Git $module @("commit", "-q", "--allow-empty", "-m", "module")
        Invoke-Git $repo @("init", "-q", "-b", "main")
        Invoke-Git $repo @("config", "core.autocrlf", "false")
        $excludes = Join-Path $repo ".git\empty-excludes"
        Set-Content -LiteralPath $excludes -Value "" -NoNewline
        Invoke-Git $repo @("config", "core.excludesFile", ($excludes -replace "\\", "/"))
        Set-Content -LiteralPath (Join-Path $repo "file.txt") -Value "one"
        Invoke-Git $repo @("add", "-A")
        Invoke-Git $repo @("commit", "-q", "-m", "first")
        Invoke-Git $repo @("submodule", "--quiet", "add", ($module -replace "\\", "/"), "module")
        Invoke-Git $repo @("commit", "-q", "-m", "second")
        Set-Content -LiteralPath (Join-Path $repo "file.txt") -Value "two"
        $app = Join-Path $root "gall.exe"
        Copy-Item -LiteralPath $source -Destination $app
        Set-Content -LiteralPath (Join-Path $root "portable") -Value "" -NoNewline
        $env:GALL_MULTI_INSTANCE = "1"; $env:GALL_SELFTEST = "popup-placement@$label"; $env:GALL_SELFTEST_DIR = $out
        Write-Output "RUN   popup-placement at $label"
        $process = Start-Process -FilePath $app -ArgumentList @("-p", "`"$data`"", "-p2", "`"$repo`"") -PassThru -RedirectStandardOutput (Join-Path $root "stdout.log") -RedirectStandardError (Join-Path $root "stderr.log")
        if (!$process.WaitForExit($TimeoutSeconds * 1000)) {
            Stop-Process -Id $process.Id -Force
            Write-Output "FAIL  timed out at $label"; $failed = $true; continue
        }
        $report = Join-Path $out "report.txt"
        if (!(Test-Path -LiteralPath $report)) { Write-Output "FAIL  no report at $label; inspect $root\stderr.log"; $failed = $true; continue }
        Get-Content -LiteralPath $report -Encoding UTF8
        if (!(Select-String -LiteralPath $report -Pattern "^0 failure" -Quiet)) { $failed = $true }
    }
} finally {
    # Newer PowerShell sets an empty variable for a null value.
    foreach ($name in $names) { if ($null -eq $previous[$name]) { Remove-Item -Path "Env:$name" -ErrorAction Ignore } else { Set-Item -Path "Env:$name" -Value $previous[$name] } }
    Write-Output "Screenshots and reports: $results"
}
if ($failed) { throw "The popup placement self-test failed." }