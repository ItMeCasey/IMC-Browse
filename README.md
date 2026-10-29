# IMC Browse

A fast, private Windows browser built in Rust on embedded Chromium (CEF). It blocks ads and trackers natively, runs Chrome **and** Firefox extensions, keeps every page in Chromium's sandbox, and stays out of your way: no account, no sync, no telemetry.

> Status: early. Windows 10/11, 64-bit. Builds and releases are produced by GitHub Actions; see [Releases](https://github.com/ItMeCasey/IMC-Browse/releases).

## What it does

**Speed and size**
- Starts fast and idles small: unused tabs hibernate (a "Zzz" badge; they wake when you click them), and tabs playing sound are never put to sleep.
- Native ad and tracker blocking (EasyList, EasyPrivacy and friends), HTTPS-first, no background phoning home.

**Security**
- Chromium's sandbox is on: web pages run at Untrusted integrity, the GPU process at Low.
- Zero ambient permissions: camera, microphone, location and the rest are denied.
- Ghost tabs: private tabs that live in memory only and are never saved, restored or put in history.
- DNS-over-HTTPS toggle, proxy / VPN with a kill switch.
- Passwords and cards are encrypted with Windows DPAPI. Fill is manual, HTTPS-only, and only on the exact site a login was saved for.
- Updates are Ed25519-signed and hash-checked before anything is replaced. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [SECURITY.md](SECURITY.md).

**Extensions (Chrome and Firefox)**
- Add from the Chrome Web Store or Firefox Add-ons: on a store page the install button becomes **Add to IMC Browse**.
- Older (Manifest V2) Chrome extensions and Firefox add-ons are converted to the current format when installed. This works for most, not all.
- Toolbar buttons open the extension's popup in a panel under the button; options open from **Settings → Extensions**, where you can also turn extensions off, remove them, or install from an address or file.

**Everyday browser**
- Tabs: drag to reorder, drag out to a new window, drag between windows; vertical tabs; restore tabs on start (also after a crash).
- History (Ctrl+H, `imc://history`) with address-bar suggestions, bookmarks with import from Chrome/Edge/Brave/Vivaldi/HTML, downloads manager, per-site zoom, mouse gestures, mute, themes.
- Registers as a Windows browser and offers to become the default.
- Optional AI side panel: it opens the AI website you choose with your own login (no API keys), and sends a page or selection only when you click.
- CLI control for AI agents (off by default): `imc-browse ctl open <url>`, `ctl text`, `ctl click ...`. See Settings → AI control.

## Install

Download the latest zip from [Releases](https://github.com/ItMeCasey/IMC-Browse/releases), unzip anywhere, run `imc-browse.exe`. The browser updates itself (Settings → Updates); turn on **Nightly** to follow the rolling pre-release.

Versions are `YYYY.M.N` (year, month, increment), for example `2026.10.1`.

## Build from source

Prerequisites: Rust (MSVC toolchain) and the MSVC build tools.

```powershell
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

# one-time: download the Chromium runtime that matches the cef crate (see Cargo.lock)
cargo install export-cef-dir --version "156.2.0+156.0.14" --locked
export-cef-dir --force "$HOME/.local/share/cef"
$env:CEF_PATH = "$HOME/.local/share/cef"

cargo build --release
pwsh tools/stage-cef.ps1 -CefPath "$HOME/.local/share/cef" -Dll target/release/imc_browse.dll -Out dist
.\dist\imc-browse.exe https://example.com
```

The app is a DLL (`imc-browse.dll`) loaded by CEF's own launcher (`imc-browse.exe`, a renamed `bootstrap.exe`): that is what lets the Chromium sandbox run. `cargo run` therefore does nothing useful; use the staged `dist` folder. Test-only hooks (`IMC_SELFTEST*`) exist only when built with `--features devhooks`.

Your data lives in `%LOCALAPPDATA%\IMC-Browse` (`config.json`, `cef-profile\`, `extensions\`, `lists\`, `history.json`, ...).

## Command line

```powershell
imc-browse.exe [URL...]
imc-browse.exe --update-lists                       # refresh the ad and tracker lists
imc-browse.exe --install-extension <store page | id | .crx | .xpi | .zip | https URL | folder>
imc-browse.exe --list-extensions
imc-browse.exe --remove-extension <folder>
imc-browse.exe ctl help                             # AI control (turn it on in Settings first)
```

## Releases and updates

- `ci.yml` builds and tests every push and pull request, and launches the browser as a smoke test.
- `nightly.yml` publishes a signed `nightly` pre-release when something changed (new commits, or new versions of our crates including Chromium).
- `release.yml` publishes a signed release when you push a tag such as `v2026.10.1` (the tag must match `version` in `Cargo.toml`).
- The private signing key is only a GitHub Actions secret (`IMC_UPDATE_KEY`); the public half is compiled into the browser.

## Licence

No licence has been chosen yet, so all rights are reserved for now.
