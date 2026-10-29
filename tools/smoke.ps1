# Launch a browser build on a tiny local page and check its window really opens and shows the
# page title. Used by CI; also handy locally:  pwsh tools/smoke.ps1 -Exe target/release/imc-browse.exe
param(
    [Parameter(Mandatory)][string]$Exe,
    [int]$TimeoutSec = 60
)
$ErrorActionPreference = 'Stop'

$dir = Join-Path ([IO.Path]::GetTempPath()) ("imc-smoke-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $dir | Out-Null
$page = Join-Path $dir 'smoke.html'
'<!doctype html><title>IMC smoke test</title><body>ok</body>' | Set-Content $page -Encoding utf8
$url = 'file:///' + ($page -replace '\\', '/')

$p = Start-Process -FilePath (Resolve-Path $Exe) -ArgumentList $url -PassThru
$title = ''
$deadline = (Get-Date).AddSeconds($TimeoutSec)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
    if ($p.HasExited) { break }
    $p.Refresh()
    $title = $p.MainWindowTitle
    if ($title -like '*IMC smoke test*') { break }
}
$exited = $p.HasExited
if (-not $exited) { taskkill /PID $p.Id /T /F | Out-Null }
Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue

if ($title -like '*IMC smoke test*') {
    Write-Host "OK: window title is '$title'"
    exit 0
}
Write-Host "Smoke test FAILED: process exited early = $exited, window title = '$title'"
exit 1
