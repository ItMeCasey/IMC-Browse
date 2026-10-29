# Architecture

IMC Browse is a Windows browser written in Rust on **CEF** (the Chromium Embedded Framework). Rust owns the window, tabs, settings, blocking, storage and updates; Chromium renders pages.

## Process layout

```
imc-browse.exe        CEF's bootstrap launcher (renamed). Sets up the Chromium sandbox, loads the app.
imc-browse.dll        The app (this repository, Rust cdylib). Exports RunWinMain.
libcef.dll + runtime  The Chromium runtime (downloaded at build time, not in git).
```

Every process (browser, renderer, GPU, utility) is `imc-browse.exe`; the launcher hands each one the sandbox information and the DLL decides, in `run()`, whether it is the browser or a helper (`execute_process`). Renderers run at Untrusted integrity and the GPU process at Low. The browser process additionally restricts where DLLs load from and joins a Job Object so nothing it starts can outlive it.

## Windows and tabs

- `tao` creates native windows (frameless, our own title bar). Each window has a **chrome UI browser** (a local `chrome.html`: tab strip, toolbar, settings) and one child **page browser** per tab, stacked above it.
- `Shell` in `src/lib.rs` is the single owner of state. Several windows are supported: each `Win` has its own tabs, active tab, panel state and UI browser. Events from a window's UI are routed to that window; tabs can move between windows by re-parenting their browser window.
- CEF's loop is pumped from tao's event loop about every 10 ms (`pump_cef`). Messages for tao's hidden thread window are held while CEF's loop runs, because tao panics if its event handler is re-entered.
- The UI talks to Rust with `console.debug("IMCIPC:...")` lines that only the UI browser's handler accepts. Rust talks to the UI with `execute_java_script`.

## Pages talking to the browser

A few features need a page to signal the browser (gestures, zoom, favicon, audio, "send page to AI", extension install on a store page). They use prefixed console lines too, but **any web page can print those**, so each one is checked on the Rust side: gestures and zoom only count right after real mouse/Ctrl input, page text is accepted only after the user clicked "Send page", an install request is honoured only if the tab really is on a store page (and installs *that* page), and history changes only from `imc://history`.

## Extensions

CEF's Alloy runtime loads unpacked extensions given with `--load-extension`; there is no embedder API for them in this CEF version. So:

1. **Installing** (`src/ext.rs`) downloads a CRX/XPI/zip, unpacks it (zip-bomb and path-traversal checks), converts Manifest V2 to V3 when needed (background scripts or page become a service worker; browser_action becomes action; permissions are split), and adds a `browser` to `chrome` alias for Firefox add-ons.
2. **Ids**: an extension's id comes from its manifest `key` when it has one (everything from the Chrome Web Store), otherwise from the folder path.
3. **Popups** open in a panel under the toolbar button (a child browser, closes when focus leaves). The engine's `tabs` API does not know our tabs, so the popup page is given the page it was opened on and answers `tabs.query` with it.
4. **Store pages**: injected script turns "Add to Chrome/Firefox" into "Add to IMC Browse".

Known limits: APIs that need the engine's own tab model (for example `tabs.sendMessage` to a content script) may not work; background pages become service workers; persistent background pages and blocking `webRequest` are not available.

## Blocking and privacy

- `src/adblock.rs`: a native filter-list engine (network rules and cosmetic filters) applied in the request handler; built-in baseline plus downloadable lists.
- HTTPS-first, DNS-over-HTTPS (written to the engine profile before launch), proxy/VPN with a kill switch that fails closed.
- Ghost tabs use an in-memory request context; nothing about them is written (history, session, favicons, downloads list).

## Storage

`%LOCALAPPDATA%\IMC-Browse`: `config.json`, `bookmarks.json`, `history.json`, `session.json`, `passwords.json` and `cards.json` (secrets encrypted with DPAPI and per-file entropy), `lists\`, `extensions\`, `cef-profile\`. Nothing is synced or sent anywhere.

## Updates

`src/updater.rs`: releases carry `update.json` and `update.json.sig` (Ed25519). The browser verifies the signature with a compiled-in public key, checks the channel, that the version is newer, and the file's SHA-256 and size, then replaces `imc-browse.dll` (a running DLL can be renamed, not overwritten) and asks for a restart. A new Chromium requires a full download.

## Control for AI tools

`src/control.rs`: a named pipe (owner-only ACL, off by default) used by `imc-browse ctl`. Ghost tabs are invisible to it; running JavaScript is a second switch; every request is logged (`imc://ai`). There are no commands for passwords or cards.

## Build and release

See the README. CI builds the DLL, stages `dist`, smoke-tests it, and the release workflow signs and publishes. The version is `YYYY.M.N` and the tag must match `Cargo.toml`.
