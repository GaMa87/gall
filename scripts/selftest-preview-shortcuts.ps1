param(
    [string]$Binary = (Join-Path $PSScriptRoot "..\target\debug\gall.exe")
)

$ErrorActionPreference = "Stop"
$source = (Resolve-Path $Binary).Path
$root = Join-Path ([System.IO.Path]::GetTempPath()) ("gall-preview-shortcuts-" + [guid]::NewGuid().ToString("N"))
$app = Join-Path $root "gall.exe"
$data = Join-Path $root "TestData"
$out = Join-Path $root "out"
$target = Join-Path $data "Target folder"
New-Item -ItemType Directory -Path $target, $out, (Join-Path $data "Projects"), (Join-Path $root "UserData") | Out-Null
Copy-Item -LiteralPath $source -Destination $app
Set-Content -LiteralPath (Join-Path $root "portable") -Value "" -NoNewline
Set-Content -LiteralPath (Join-Path $target "target.2do") -Value "Shortcut target note"
Set-Content -LiteralPath (Join-Path $data "broken.pdf") -Value "Not a PDF"

$pdf = "%PDF-1.4`n"
$offsets = [System.Collections.Generic.List[int]]::new()
$objects = @(
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >>",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"
)
foreach ($page in 1, 2) {
    $stream = "BT /F1 22 Tf 40 400 Td (Gall PDF page $page) Tj ET`n"
    $objects += "<< /Length $($stream.Length) >>`nstream`n${stream}endstream"
}
for ($i = 0; $i -lt $objects.Count; $i++) {
    $offsets.Add($pdf.Length)
    $pdf += "$($i + 1) 0 obj`n$($objects[$i])`nendobj`n"
}
$xref = $pdf.Length
$pdf += "xref`n0 8`n0000000000 65535 f `n"
foreach ($offset in $offsets) { $pdf += $offset.ToString("D10") + " 00000 n `n" }
$pdf += "trailer`n<< /Size 8 /Root 1 0 R >>`nstartxref`n$xref`n%%EOF`n"
[System.IO.File]::WriteAllText((Join-Path $data "sample.pdf"), $pdf, [System.Text.Encoding]::ASCII)

$shell = New-Object -ComObject WScript.Shell
foreach ($fixture in @(
    @("Folder shortcut.lnk", $target),
    @("Note shortcut.lnk", (Join-Path $target "target.2do")),
    @("Broken shortcut.lnk", (Join-Path $data "missing.txt"))
)) {
    $link = $shell.CreateShortcut((Join-Path $data $fixture[0]))
    $link.TargetPath = $fixture[1]
    $link.Save()
}
$names = @("GALL_MULTI_INSTANCE", "GALL_SELFTEST", "GALL_SELFTEST_DIR")
$previous = @{}
foreach ($name in $names) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, "Process") }
try {
    $env:GALL_MULTI_INSTANCE = "1"
    $env:GALL_SELFTEST = "preview-shortcuts"
    $env:GALL_SELFTEST_DIR = $out
    $process = Start-Process -FilePath $app -ArgumentList @("-p", "`"$data`"", "-p2", "`"$(Join-Path $data 'Projects')`"") -PassThru -RedirectStandardOutput (Join-Path $root "stdout.log") -RedirectStandardError (Join-Path $root "stderr.log")
    if (!$process.WaitForExit(120000)) {
        Stop-Process -Id $process.Id
        throw "Preview/shortcut self-test timed out. Artifacts: $root"
    }
    $report = Join-Path $out "report.txt"
    if (!(Test-Path -LiteralPath $report)) { throw "No self-test report. Inspect $root\stderr.log" }
    Get-Content -LiteralPath $report
    Write-Output "Artifacts: $root"
    if (!(Select-String -LiteralPath $report -Pattern "^0 failure" -Quiet)) { throw "Preview/shortcut self-test failed. Artifacts: $root" }
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $previous[$name], "Process") }
}
