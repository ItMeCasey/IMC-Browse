# Assemble a runnable CEF build: the CEF runtime files plus our exe in one folder.
#   pwsh tools/stage-cef.ps1 -CefPath ~/.local/share/cef -Exe cef/target/release/imc-browse-cef.exe -Out cef/dist
param(
    [Parameter(Mandatory)][string]$CefPath,
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Force $Out | Out-Null
Get-ChildItem $CefPath -File |
    Where-Object { $_.Extension -in '.dll', '.pak', '.dat', '.bin', '.json' -and $_.Name -ne 'archive.json' } |
    Copy-Item -Destination $Out -Force
Copy-Item (Join-Path $CefPath 'locales') $Out -Recurse -Force
Copy-Item $Exe $Out -Force
$mb = [math]::Round((Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum / 1MB)
Write-Host "Staged CEF build in $Out ($mb MB)"
