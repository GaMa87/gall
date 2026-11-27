<#
Runs the graphical Git self-tests (git-workspace and git-compare; git-baseline counts steps and measures latencies) on a bundled debug build on Windows, each at
100, 125, 150 and 200 % (derived from the display's own scale, as selftest-rendering.ps1 does), in a throw-away
portable profile with disposable repositories. Unattended-safe: it refuses to start on a locked session, keeps the
machine awake while it runs, writes summary.json next to the PNGs, and exits non-zero on any failure.

  npm --prefix ui run build; cargo build -p gall --features tauri/custom-protocol
  scripts\selftest-git.ps1 [-Binary target\debug\gall.exe] [-Scenario git-workspace,git-compare,diff-permission]
                           [-Scales 1,1.25,1.5,2] [-OutputFolder <folder>] [-Wsl [-WslStrict]] [-KeepFixtures]

-Wsl adds a separate git-workspace run with the repository inside WSL (\\wsl.localhost\<distro>\...), which exercises
the gitworker shell-pool path. It is skipped, with a reason in the summary, when WSL or git inside it is missing.
The output folder is printed at the end; open every PNG.
#>
param(
    [string]$Binary = (Join-Path $PSScriptRoot "..\target\debug\gall.exe"),
    [string[]]$Scenario = @("git-workspace", "git-compare"),
    [double[]]$Scales = @(1, 1.25, 1.5, 2),
    [string]$OutputFolder = "",
    [switch]$Wsl,
    [switch]$KeepFixtures,
    [switch]$WslStrict,
    [int]$TimeoutSeconds = 300
)

$ErrorActionPreference = "Stop"
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class GallSelftestNative {
    [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint state);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern uint GetLongPathName(string shortPath, StringBuilder buffer, uint size);
}
"@

function Test-SessionUnlocked {
    # A locked or disconnected session switches the input desktop to the secure one, which cannot be opened.
    $desktop = [GallSelftestNative]::OpenInputDesktop(0, $false, 0x0100)
    if ($desktop -eq [IntPtr]::Zero) { return $false }
    [void][GallSelftestNative]::CloseDesktop($desktop)
    return $true
}

function Get-LongPath([string]$path) {
    # Temp folders can come as 8.3 names (MATEJ~1); the app and Git must see the same spelling.
    $buffer = New-Object System.Text.StringBuilder 1024
    $length = [GallSelftestNative]::GetLongPathName($path, $buffer, 1024)
    if ($length -gt 0 -and $length -lt 1024) { return $buffer.ToString() }
    return $path
}

function Write-LfFile([string]$path, [string]$text) {
    [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false)))
}

# Hooks, user configuration, the system configuration and signing never reach the fixtures.
$script:EmptyConfig = ""
function Invoke-Git {
    param([string]$Dir, [Parameter(ValueFromRemainingArguments = $true)][string[]]$GitArgs)
    $all = @("-C", $Dir, "-c", "core.autocrlf=false", "-c", "core.hooksPath=NUL", "-c", "commit.gpgSign=false", "-c", "protocol.file.allow=always") + $GitArgs
    $output = & git @all 2>&1
    if ($LASTEXITCODE -ne 0) { throw "git $($GitArgs -join ' ') failed in ${Dir}: $output" }
    return $output
}

function New-Repo([string]$dir, [string]$branch = "main") {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    & git init -q -b $branch $dir
    if ($LASTEXITCODE -ne 0) { throw "git init failed in $dir" }
    Invoke-Git $dir config core.autocrlf false
    Invoke-Git $dir config user.name Test
    Invoke-Git $dir config user.email test@example.com
    Invoke-Git $dir config commit.gpgSign false
    Invoke-Git $dir config core.hooksPath NUL
    $excludes = Join-Path $dir ".git\empty-excludes"
    Write-LfFile $excludes ""
    Invoke-Git $dir config core.excludesFile ($excludes -replace "\\", "/")
}

function Set-Line([string]$file, [int]$number, [string]$text) {
    $lines = [System.IO.File]::ReadAllLines($file)
    $lines[$number - 1] = $text
    Write-LfFile $file (($lines -join "`n") + "`n")
}

function Add-Commit([string]$dir, [string]$message) { Invoke-Git $dir add -A | Out-Null; Invoke-Git $dir commit -q -m $message | Out-Null }

# Not Git: the grouping of the file list by type (`-Scenario file-groups`), which needs the same scales and checks.
function New-FileGroupsFixture([string]$root) {
    $td = Join-Path $root "td"
    foreach ($dir in "Projects", "mixed\src", "mixed\Docs", "empty", "single", "many") { New-Item -ItemType Directory -Force -Path (Join-Path $td $dir) | Out-Null }
    $day = [datetime]::new(2026, 1, 1)
    # Name, size, day of the last change.
    foreach ($file in @(@("b.PNG", 100, 20), @("a.png", 250, 10), @("c10.png", 50, 30), @("c2.png", 50, 30), @("notes.txt", 10, 50), @("read.txt", 700, 5), @("x.md", 5, 60),
            @("pack.tar.gz", 2000, 25), @(".bashrc", 7, 15), @("Makefile", 9, 8), @("LICENSE", 3, 2), @("noext", 1, 3))) {
        $path = Join-Path $td ("mixed\" + $file[0])
        Write-LfFile $path ("x" * $file[1])
        [System.IO.File]::SetLastWriteTime($path, $day.AddDays($file[2]))
    }
    foreach ($i in 1..5) { Write-LfFile (Join-Path $td "single\file$i.txt") "x" }
    foreach ($i in 1..300) { Write-LfFile (Join-Path $td ("many\file.x{0:000}" -f $i)) "x" }
}

function New-GitWorkspaceFixture([string]$root) {
    $td = Join-Path $root "td"
    New-Item -ItemType Directory -Force -Path (Join-Path $td "Projects") | Out-Null
    New-Repo $td
    Write-LfFile (Join-Path $td "file.txt") ((1..30 | ForEach-Object { "$_" }) -join "`n" | ForEach-Object { $_ + "`n" })
    Add-Commit $td "initial file"
    $base = (Invoke-Git $td rev-parse HEAD).Trim()
    foreach ($name in "alpha", "beta", "gamma") { Write-LfFile (Join-Path $td "$name.txt") "$name`n"; Add-Commit $td "add $name" }
    foreach ($side in @(@("conflict-left", "left side", "left conflict"), @("conflict-right", "right side", "right conflict"))) {
        Invoke-Git $td checkout -q -b $side[0] $base | Out-Null
        Set-Line (Join-Path $td "file.txt") 15 $side[1]
        Invoke-Git $td commit -q -am $side[2] | Out-Null
    }
    Invoke-Git $td checkout -q main | Out-Null
    $remote = Join-Path $root "remote.git"
    & git clone --bare -q $td $remote
    if ($LASTEXITCODE -ne 0) { throw "git clone --bare failed" }
    Invoke-Git $remote "--git-dir=$remote" config core.autocrlf false
    Invoke-Git $td remote add origin $remote | Out-Null
    Invoke-Git $td fetch -q origin | Out-Null
    Invoke-Git $td branch --set-upstream-to=origin/main main | Out-Null
    $host_ = Join-Path $root "submodule-host"
    New-Repo $host_
    Invoke-Git $host_ submodule add -q $remote module | Out-Null
    Invoke-Git $host_ config -f .gitmodules submodule.module.path module | Out-Null
    Invoke-Git ([IO.Path]::Combine($host_, "module")) config core.autocrlf false | Out-Null
    Invoke-Git $host_ commit -q -m "add submodule" | Out-Null
    Invoke-Git $host_ submodule deinit -q module | Out-Null
    # More than one page of history, so the "Load more commits" button is shown.
    $many = Join-Path $root "many-commits"
    New-Repo $many
    $stream = New-Object System.Text.StringBuilder
    foreach ($i in 1..230) {
        [void]$stream.Append(("commit refs/heads/main`ncommitter Test <test@example.com> {0} +0000`ndata 15`ncommit no. {1:000}`n`n" -f (1700000000 + $i * 60), $i))
    }
    $streamFile = Join-Path $root "many-commits.stream"
    Write-LfFile $streamFile $stream.ToString()
    # The stream is fed from a file: piping text through PowerShell would add a byte-order mark or CRLF.
    $fast = Start-Process -FilePath "git" -ArgumentList @("-C", "`"$many`"", "fast-import", "--quiet") -RedirectStandardInput $streamFile `
        -RedirectStandardError (Join-Path $root "fast-import.err") -NoNewWindow -Wait -PassThru
    if ($fast.ExitCode -ne 0) { throw "git fast-import failed: $(Get-Content -LiteralPath (Join-Path $root 'fast-import.err') -Raw)" }
    Invoke-Git $many reset -q --hard | Out-Null
    Set-Line (Join-Path $td "file.txt") 2 "second changed"
    Set-Line (Join-Path $td "file.txt") 25 "twenty-five changed"
    Write-LfFile (Join-Path $td "new.txt") "untracked`n"
}

function New-GitCompareFixture([string]$root) {
    $td = Join-Path $root "td"
    New-Item -ItemType Directory -Force -Path (Join-Path $td "Projects") | Out-Null
    New-Repo $td
    Write-LfFile (Join-Path $td "before.txt") "first`n"; Add-Commit $td "first"
    Invoke-Git $td mv before.txt file.txt | Out-Null; Add-Commit $td "second"
    Write-LfFile (Join-Path $td "file.txt") "first`r`n"; Add-Commit $td "convert line endings"
    $initial = (Invoke-Git $td symbolic-ref --short HEAD).Trim()
    Invoke-Git $td checkout --detach -q HEAD | Out-Null
    Write-LfFile (Join-Path $td "reflog.txt") "reflog only`n"; Add-Commit $td "reflog-only commit"
    Invoke-Git $td checkout -q $initial | Out-Null
    Invoke-Git $td commit --allow-empty -qm "unrelated commit" | Out-Null
    Write-LfFile (Join-Path $td "file.txt") "working`n"
    Write-LfFile (Join-Path $td "new.txt") "untracked`n"
}

function New-DiffPermissionFixture([string]$root) {
    $td = Join-Path $root "td"
    New-Item -ItemType Directory -Force -Path (Join-Path $td "Projects") | Out-Null
    Write-LfFile (Join-Path $td "left.txt") "alpha`nbeta`n"
    Write-LfFile (Join-Path $td "right.txt") "alpha`nBETA`n"
    Write-LfFile (Join-Path $td "other-left.txt") "one`n"
    Write-LfFile (Join-Path $td "other-right.txt") "two`n"
}

function New-GitBaselineFixture([string]$root) {
    $td = Join-Path $root "td"
    New-Item -ItemType Directory -Force -Path (Join-Path $td "Projects") | Out-Null
    & python3 (Join-Path $PSScriptRoot "selftest_git_baseline_fixture.py") (Join-Path $td "repos")
    if ($LASTEXITCODE -ne 0) { throw "selftest_git_baseline_fixture.py failed (Python 3 is required)" }
}

function Read-Report([string]$report, [string]$scenario, [double]$scale, [string]$out) {
    $rows = @()
    foreach ($line in Get-Content -LiteralPath $report -Encoding UTF8) {
        if ($line -match '^(PASS|FAIL)\s+(.*)$') {
            $rows += [ordered]@{ scenario = $scenario; scale = $scale; status = $Matches[1]; assertion = $Matches[2] }
        }
    }
    $shots = @(Get-ChildItem -LiteralPath $out -Filter *.png -ErrorAction SilentlyContinue | Sort-Object Name | ForEach-Object FullName)
    return @{ rows = $rows; shots = $shots }
}

function Invoke-App {
    param([string]$App, [string]$Root, [string]$Mode, [string]$Out, [string]$Data)
    $names = "GALL_MULTI_INSTANCE", "GALL_SELFTEST", "GALL_SELFTEST_DIR", "GALL_SELFTEST_CAPTURE", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM",
        "GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL"
    $previous = @{}
    foreach ($name in $names) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, "Process") }
    try {
        $env:GALL_MULTI_INSTANCE = "1"; $env:GALL_SELFTEST = $Mode; $env:GALL_SELFTEST_DIR = $Out
        $env:GIT_CONFIG_GLOBAL = $script:EmptyConfig; $env:GIT_CONFIG_NOSYSTEM = "1"
        $env:GIT_AUTHOR_NAME = "Test"; $env:GIT_AUTHOR_EMAIL = "test@example.com"
        $env:GIT_COMMITTER_NAME = "Test"; $env:GIT_COMMITTER_EMAIL = "test@example.com"
        $td = Join-Path $Root "td"
        $process = Start-Process -FilePath $App -ArgumentList @("-p", "`"$td`"", "-p2", "`"$(Join-Path $td 'Projects')`"") -PassThru `
            -RedirectStandardOutput (Join-Path $Root "stdout.log") -RedirectStandardError (Join-Path $Root "stderr.log")
        if (!$process.WaitForExit($TimeoutSeconds * 1000)) {
            Stop-Process -Id $process.Id -Force
            return "timeout"
        }
        return "exited"
    } finally {
        # Newer PowerShell sets an empty variable for a null value, and Git refuses an empty author name.
        foreach ($name in $names) { if ($null -eq $previous[$name]) { Remove-Item -Path "Env:$name" -ErrorAction Ignore } else { Set-Item -Path "Env:$name" -Value $previous[$name] } }
    }
}

if (!(Test-SessionUnlocked)) {
    Write-Error "The Windows session is locked or disconnected: screenshots of a locked desktop are black. Unlock it (and keep it connected, e.g. use tscon instead of closing a remote session) and run again."
    exit 2
}
# One graphical self-test at a time on this desktop: two runs take the clipboard, the focus and the screen from each other.
$desktopLock = [System.Threading.Mutex]::new($false, "Local\GallGraphicalSelftest")
try { $locked = $desktopLock.WaitOne(0) } catch [System.Threading.AbandonedMutexException] { $locked = $true }
if (!$locked) {
    Write-Output "WAIT  another graphical self-test is running on this desktop"
    try { $locked = $desktopLock.WaitOne([TimeSpan]::FromMinutes(30)) } catch [System.Threading.AbandonedMutexException] { $locked = $true }
    if (!$locked) { Write-Error "Another graphical self-test still runs after 30 minutes."; exit 2 }
}
$source = (Resolve-Path $Binary).Path
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$results = if ($OutputFolder) { $OutputFolder } else { Join-Path ([System.IO.Path]::GetTempPath()) "gall-git-selftest-$stamp" }
New-Item -ItemType Directory -Force -Path $results | Out-Null
$results = Get-LongPath (Resolve-Path $results).Path
$work = Get-LongPath (New-Item -ItemType Directory -Path (Join-Path ([System.IO.Path]::GetTempPath()) ("gall-git-fixture-" + [guid]::NewGuid().ToString("N")))).FullName
$script:EmptyConfig = Join-Path $work "empty.gitconfig"
Write-LfFile $script:EmptyConfig ""
$app = Join-Path $work "gall.exe"
Copy-Item -LiteralPath $source -Destination $app
Set-Content -LiteralPath (Join-Path $work "portable") -Value "" -NoNewline

$allRows = @(); $runs = @(); $skipped = @(); $failed = $false
$ES_CONTINUOUS = [uint32]2147483648; $ES_SYSTEM = [uint32]1; $ES_DISPLAY = [uint32]2
[void][GallSelftestNative]::SetThreadExecutionState($ES_CONTINUOUS -bor $ES_SYSTEM -bor $ES_DISPLAY)
try {
    $plan = @()
    foreach ($name in $Scenario) { foreach ($scale in $Scales) { $plan += , @($name, $scale, "native") } }
    if ($Wsl) {
        $distro = $null
        $wslReason = $null
        if (!(Get-Command wsl.exe -ErrorAction SilentlyContinue)) { $wslReason = "wsl.exe is not installed" }
        else {
            $listed = ((& wsl.exe -l -q) | ForEach-Object { ($_ -replace "`0", "").Trim() } | Where-Object { $_ })
            if (@($listed).Count -eq 0) { $wslReason = "no WSL distribution is installed" }
            foreach ($candidate in $listed) {
                & wsl.exe -d $candidate -e git --version *> $null
                if ($LASTEXITCODE -eq 0) { $distro = $candidate; break }
            }
            if (!$distro -and !$wslReason) { $wslReason = "git is not installed in any WSL distribution ($($listed -join ", "))" }
        }
        if ($distro) { $plan += , @("git-workspace", $Scales[[Math]::Min(2, $Scales.Count - 1)], "wsl") }
        else { $skipped += [ordered]@{ scenario = "git-workspace"; variant = "wsl"; reason = $wslReason }; Write-Output "SKIP  WSL variant: $wslReason" }
    }
    foreach ($item in $plan) {
        $name, $scale, $variant = $item
        $tag = "$name-$variant-$($scale.ToString([cultureinfo]::InvariantCulture) -replace '\.', '_')"
        $root = Join-Path $work $tag
        New-Item -ItemType Directory -Force -Path $root | Out-Null
        $out = Join-Path $results $tag
        New-Item -ItemType Directory -Force -Path $out | Out-Null
        Write-Output "RUN   $name at $scale ($variant)"
        $status = "failed"
        $failedBefore = $failed
        $failed = $false
        try {
            if ($variant -eq "wsl") {
                $wslRoot = "/tmp/gall-selftest-$stamp"
                $fixtureScript = (Resolve-Path (Join-Path $PSScriptRoot "selftest-git-wsl-fixture.sh")).Path
                $fixtureWsl = (& wsl.exe -d $distro -e wslpath -a ($fixtureScript -replace "\\", "/")).Trim()
                & wsl.exe -d $distro -e bash $fixtureWsl $wslRoot
                if ($LASTEXITCODE -ne 0) { throw "WSL fixture failed" }
                $root = "\\wsl.localhost\$distro" + ($wslRoot -replace "/", "\")
                New-Item -ItemType Directory -Force -Path (Join-Path $work "wsl-profile") | Out-Null
            } elseif ($name -eq "git-workspace") { New-GitWorkspaceFixture $root }
            elseif ($name -eq "git-compare") { New-GitCompareFixture $root }
            elseif ($name -eq "diff-permission") { New-DiffPermissionFixture $root }
            elseif ($name -eq "git-baseline") { New-GitBaselineFixture $root }
            elseif ($name -eq "file-groups") { New-FileGroupsFixture $root }
            else { throw "Unknown scenario $name" }
            $mode = "$name@" + $scale.ToString([cultureinfo]::InvariantCulture)
            # A fresh profile per run whose file index covers only the fixture: the default scope is the whole home
            # folder, and indexing a million files competes with the Git operations the scenario times.
            $userData = Join-Path $work "UserData"
            if (Test-Path -LiteralPath $userData) { Remove-Item -LiteralPath $userData -Recurse -Force }
            New-Item -ItemType Directory -Path $userData | Out-Null
            Write-LfFile (Join-Path $userData "settings.json") (@{ indexRoots = @((Join-Path $root "td")) } | ConvertTo-Json -Compress)
            if ($variant -eq "wsl") {
                # The profile and logs stay on Windows; only the repositories live in WSL.
                $profileRoot = Join-Path $work "wsl-profile"
                Copy-Item -LiteralPath (Join-Path $work "portable") -Destination (Join-Path $profileRoot "portable") -Force
                $state = Invoke-App -App $app -Root $root -Mode $mode -Out $out -Data $profileRoot
            } else {
                $state = Invoke-App -App $app -Root $root -Mode $mode -Out $out -Data $root
            }
            $report = Join-Path $out "report.txt"
            if ($state -eq "timeout") { Write-Output "FAIL  $tag timed out"; $failed = $true; $status = "timeout" }
            elseif (!(Test-Path -LiteralPath $report)) { Write-Output "FAIL  $tag produced no report (see stderr.log)"; $failed = $true; $status = "no-report" }
            else {
                $parsed = Read-Report $report $name $scale $out
                $allRows += $parsed.rows
                $bad = @($parsed.rows | Where-Object { $_.status -eq "FAIL" })
                $zeroFailures = Select-String -LiteralPath $report -Pattern "^0 failure" -Quiet
                if ($bad.Count -gt 0 -or !$zeroFailures) { $failed = $true; $status = "failed"; $bad | ForEach-Object { Write-Output "FAIL  $tag $($_.assertion)" } }
                else { $status = "passed" }
                $blank = $parsed.shots | Where-Object { (Get-Item $_).Length -lt 2000 }
                if ($blank) { $failed = $true; $status = "blank-screenshots"; Write-Output "FAIL  $tag has near-empty screenshots: $($blank -join ', ')" }
                $runs += [ordered]@{ scenario = $name; variant = $variant; scale = $scale; status = $status; assertions = $parsed.rows.Count; screenshots = $parsed.shots; report = $report }
            }
            foreach ($log in "stdout.log", "stderr.log") { $path = Join-Path $root $log; if (Test-Path -LiteralPath $path) { Copy-Item -LiteralPath $path -Destination (Join-Path $out $log) } }
        } catch {
            $failed = $true
            Write-Output "FAIL  $tag $($_.Exception.Message)"
            $runs += [ordered]@{ scenario = $name; variant = $variant; scale = $scale; status = "error"; error = $_.Exception.Message }
        } finally {
            if (!$KeepFixtures -and $variant -eq "wsl") { & wsl.exe -d $distro -e rm -rf $wslRoot 2>$null }
        }
        if ($variant -eq "wsl" -and $failed -and !$WslStrict) {
            # The WSL variant is reported but advisory unless -WslStrict: it also covers WSL-only gaps (see docs/git-workspace-roadmap.md).
            $failed = $false
            if ($runs.Count -gt 0) { $runs[$runs.Count - 1]["advisory"] = $true }
            Write-Output "NOTE  the WSL variant failed; reported in summary.json but not counted (use -WslStrict to count it)"
        }
        $failed = $failed -or $failedBefore
        if (!(Test-SessionUnlocked)) { $failed = $true; Write-Output "FAIL  the session was locked during $tag; later runs are skipped"; break }
    }
} finally {
    [void][GallSelftestNative]::SetThreadExecutionState($ES_CONTINUOUS)
    $desktopLock.ReleaseMutex()
    $summary = [ordered]@{ generated = (Get-Date -Format o); binary = $source; passed = !$failed; runs = $runs; skipped = $skipped; assertions = $allRows }
    Write-LfFile (Join-Path $results "summary.json") ($summary | ConvertTo-Json -Depth 6)
    if (!$KeepFixtures) {
        for ($try = 0; $try -lt 3 -and (Test-Path -LiteralPath $work); $try++) {
            Get-ChildItem -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue | ForEach-Object { try { if ($_.Attributes -band [IO.FileAttributes]::ReadOnly) { $_.Attributes = "Normal" } } catch {} }
            Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
            if (Test-Path -LiteralPath $work) { Start-Sleep -Seconds 1 }
        }
    }
    Write-Output "Screenshots, reports and summary.json: $results"
}
if ($failed) { exit 1 }
exit 0
