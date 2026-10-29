# IMC Browse

Small, fast, privacy-first Windows browser: Rust + WebView2, native ad/tracker blocking, HTTPS-first, tab hibernation, deny-by-default permissions, extension installer, proxy/VPN with kill switch. Design and limits: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Build and run (Windows 10/11)

Prerequisites: Rust (MSVC toolchain) and the MSVC build tools; the WebView2 Runtime (already present with Edge).

```powershell
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
# new terminal, then:
cd IMC-Browse
cargo test
cargo build --release
.\target\release\imc-browse.exe https://example.com
```

Binary is about 1 MB. Data lives in `%LOCALAPPDATA%\IMC-Browse` (`config.json`, `profile\`, `lists\`, `extensions\`).

## Commands

```powershell
.\imc-browse.exe --update-lists                      # EasyList, EasyPrivacy, uBlock filters
.\imc-browse.exe --install-extension <id|.crx|.xpi|https-url|folder>
.\imc-browse.exe --list-extensions
.\imc-browse.exe --remove-extension <folder>
```

Chrome Web Store id example: `cjpalhdlnbpafiamejdnhcphjbkeiagm` (uBlock Origin; redundant here, but a good loader test). Restart after installing.

## Themes, tabs, bookmarks, passwords, mute

Menu button (top right) opens the panel.

- **Theme:** presets (dark, light, midnight, forest, rose, contrast, system) or custom colours; applies live.
- **Vertical tabs:** General → Vertical tabs; applies live.
- **Mute:** a speaker icon appears on any tab playing sound; click it to mute/unmute. Tabs playing audio are never put to sleep.
- **Bookmarks:** star in the toolbar (Ctrl+D). Import directly from Chrome/Edge/Brave/Vivaldi, or any browser's exported HTML.
- **Passwords:** import a CSV export (Chrome, Edge, Brave, Firefox, Bitwarden, 1Password). Stored encrypted with Windows DPAPI. Fill is manual, HTTPS-only, same-site only. Chrome's own encrypted database can't be read directly, and the CSV is plaintext, so delete it after import.

## Proxy / VPN

Gear icon → set type, host, port, kill switch, bypass list (`a.com;*.lan`) → Apply (restarts, restores tabs). You must provide the proxy endpoint. With the kill switch on, every request fails while the endpoint is unreachable.

## Config (`config.json`)

`freeze_after_secs`, `adblock`, `https_first`, `home`, `vpn{}`, and `permission_allow`: `{"https://meet.example.com": ["camera","microphone"]}`; anything not listed is denied.
