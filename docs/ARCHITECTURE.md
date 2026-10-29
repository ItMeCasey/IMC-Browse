# IMC Browse: architecture

Rust shell around the system **WebView2** (Chromium) engine. Building a rendering engine, JS engine and OS sandbox from scratch is a multi-year effort; this design owns everything *around* the engine (tabs, network filtering, permissions, extensions install, proxy, process containment) and reuses Chromium's engine and renderer sandbox.

## Files

```
Cargo.toml
data/builtin.txt        baseline ad/tracker/miner/fingerprint rules (embedded)
src/main.rs             event loop, App (tabs, layout, IPC, freezing), CLI
src/ui/chrome.html      tab strip, omnibox, shield/VPN/settings (its own webview)
src/adblock.rs          network filter engine (ABP/uBO syntax, token index)
src/winhook.rs          WebView2 COM hooks: request interception, suspend, memory level
src/ext.rs              extension installer (CRX/XPI/zip/Web Store id) + Firefox shim
src/vpn.rs              SOCKS5/HTTP proxy args, split tunnel, kill-switch watchdog
src/sandbox.rs          Windows Job Object around the whole process tree
src/config.rs           config.json + %LOCALAPPDATA%\IMC-Browse layout
```

## Processes

```
imc-browse.exe  (Job Object: KILL_ON_JOB_CLOSE)
 ├─ msedgewebview2 browser process   one per profile, shared by all webviews
 │   ├─ GPU process                  GPU raster + zero-copy forced on
 │   ├─ renderer(s)                  sandboxed, site-isolated (StrictOriginIsolation)
 │   ├─ utility / network service
 │   └─ extension background hosts   from WebView2's native extension runtime
```

- Every tab is its own WebView2 child webview; Chromium puts cross-site tabs/frames in separate sandboxed renderers, so one crashing page does not take down the browser.
- Our process is single-threaded UI + two tiny helper threads (5 s tick, 4 s proxy probe). Network filtering runs *inline* on the UI thread inside `WebResourceRequested`, so there is no queue to back up.

## Threads

| Thread | Job |
|---|---|
| main (tao event loop, COM STA) | window, webviews, request filter callback, IPC |
| tick | sends `Ev::Tick` every 5 s → hibernates idle tabs |
| vpn watchdog | TCP-probes the proxy; flips `TUNNEL_DOWN` (atomic) |

## Request path

`page → WebResourceRequested → (kill switch? → 403) → Engine::should_block → (block → 403 | continue)`.

`Engine`: rules parsed once into three indexes: host map (`||host^` rules, looked up by walking label suffixes), token map (every other rule keyed by its rarest alphanumeric token, FNV hash) and a small fallback list. A request tokenises its URL and only evaluates rules in matching buckets. Exceptions (`@@`) win. Supported options: resource types, `third-party`/`~third-party`, `domain=`. Rules needing more (`redirect=`, `csp=`, `removeparam=`, scriptlets) are skipped, not mis-applied.

## Security controls

| Control | Where | State |
|---|---|---|
| Renderer sandbox, site isolation | WebView2/Chromium | inherited |
| Job Object containment | `sandbox.rs` | done |
| Permissions deny-by-default, per-origin whitelist | `with_permission_handler` | done |
| HTTPS-first (main-frame upgrade, explicit `http://` opt-out per host) | navigation handler | done |
| Tracker/miner/fingerprinter blocking pre-fetch | `winhook.rs` | done |
| Telemetry/background services off | `BASE_ARGS` in `main.rs` | done (Chromium switches) |
| WebRTC IP-leak guard when proxy on | `vpn.rs` | done |
| Rust memory safety | whole shell | `unsafe` only in the COM/Job Object calls |

## Tab hibernation

Active tab: visible, normal memory target. Background tab: hidden + `MemoryUsageLevel::Low`. After `freeze_after_secs` idle: `ICoreWebView2_3::TrySuspend` (timers and rendering stop, Chromium trims the working set). Switching back calls `Resume`.

## Known gaps (not done, on purpose)

- **Engine/sandbox are Chromium's**, not custom. Telemetry removal is by switches; WebView2 itself still talks to Microsoft for runtime updates.
- **WireGuard is not implemented.** The "VPN" is a SOCKS5/HTTP proxy client; you supply the endpoint. No free zero-log servers are bundled, and none can be promised.
- **Subframe ad documents** are not request-blocked (document loads are exempt so navigation can't break); cosmetic hiding covers the common cases.
- **Extensions:** loaded by WebView2's native runtime, which supports a subset of `chrome.*` and no toolbar popups. Chrome Web Store/AMO packages are installed by `--install-extension`; no store UI. Firefox-only APIs beyond the `browser`→`chrome` alias are unsupported.
- No bookmarks, history UI, downloads UI, find-in-page, or incognito yet.
- Windows only (`windows`/`webview2-com`); the rest ports to WebKit/Linux/macOS with a different hooks module.
