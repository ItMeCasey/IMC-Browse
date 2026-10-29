# Assemble a runnable build: the CEF runtime files, CEF's bootstrap launcher renamed imc-browse.exe
# (it sets up the Chromium sandbox), and our app as imc-browse.dll (the launcher loads it by name).
#   pwsh tools/stage-cef.ps1 -CefPath ~/.local/share/cef -Dll target/release/imc_browse.dll -Out dist
param(
    [Parameter(Mandatory)][string]$CefPath,
    [Parameter(Mandatory)][string]$Dll,
    [Parameter(Mandatory)][string]$Out
)
$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Force $Out | Out-Null
Get-ChildItem $CefPath -File |
    Where-Object { $_.Extension -in '.dll', '.pak', '.dat', '.bin', '.json' -and $_.Name -ne 'archive.json' } |
    Copy-Item -Destination $Out -Force
Copy-Item (Join-Path $CefPath 'locales') $Out -Recurse -Force
Copy-Item (Join-Path $CefPath 'bootstrap.exe') (Join-Path $Out 'imc-browse.exe') -Force
Copy-Item $Dll (Join-Path $Out 'imc-browse.dll') -Force
$mb = [math]::Round((Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum / 1MB)
Write-Host "Staged build in $Out ($mb MB)"
